use super::*;
use chrono::Local;
const MOUSE_WHEEL_LINES: usize = 3;
const MIN_LEFT_WIDTH_PCT: u16 = 14;
const MAX_LEFT_WIDTH_PCT: u16 = 38;
const MIN_RIGHT_WIDTH_PCT: u16 = 14;
const MAX_RIGHT_WIDTH_PCT: u16 = 50;
const MIN_CENTER_WIDTH_PCT: u16 = 20;
const DOUBLE_CLICK_THRESHOLD: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseTarget {
    LeftPane,
    LeftRow(usize),
    TerminalRow(usize),
    TerminalPane,
    Center,
    FilesPane,
    UnstagedFile(Option<usize>),
    StagedFile(Option<usize>),
    CommitChrome,
    CommitText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptMouseTarget {
    CommandInput,
    CommandItem(usize),
    BrowseProjectInput,
    BrowseProjectItem(usize),
    PickEditorItem(usize),
    RuntimeKillInput,
    RuntimeKillItem(usize),
    RuntimeKillCancel,
    RuntimeKillHovered,
    RuntimeKillSelected,
    RuntimeKillVisible,
    ConfirmKillCancel,
    ConfirmKillConfirm,
    ConfirmDeleteCancel,
    ConfirmDeleteConfirm,
    ConfirmQuitCancel,
    ConfirmQuitConfirm,
    ConfirmDiscardCancel,
    ConfirmDiscardConfirm,
    RenameInput,
    NameNewAgentInput,
}

fn contains_point(rect: Rect, column: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && column >= rect.x
        && column < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

fn cursor_from_single_line_position(
    text: &str,
    text_area: Rect,
    prefix_width: usize,
    column: u16,
) -> usize {
    let relative_col = usize::from(column.saturating_sub(text_area.x));
    let target_col = relative_col
        .saturating_sub(prefix_width)
        .min(text.chars().count());
    text.char_indices()
        .nth(target_col)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn clamp_left_width_pct(left_width_pct: u16, right_width_pct: u16) -> u16 {
    let max_left = MAX_LEFT_WIDTH_PCT.min(100 - MIN_CENTER_WIDTH_PCT - right_width_pct);
    left_width_pct.clamp(MIN_LEFT_WIDTH_PCT, max_left.max(MIN_LEFT_WIDTH_PCT))
}

fn clamp_right_width_pct(right_width_pct: u16, left_width_pct: u16) -> u16 {
    let max_right = MAX_RIGHT_WIDTH_PCT.min(100 - MIN_CENTER_WIDTH_PCT - left_width_pct);
    right_width_pct.clamp(MIN_RIGHT_WIDTH_PCT, max_right.max(MIN_RIGHT_WIDTH_PCT))
}

const MIN_TERMINAL_PANE_HEIGHT_PCT: u16 = 10;
const MAX_TERMINAL_PANE_HEIGHT_PCT: u16 = 80;
const MIN_STAGED_PANE_HEIGHT_PCT: u16 = 10;
const MAX_STAGED_PANE_HEIGHT_PCT: u16 = 80;
const MIN_COMMIT_PANE_HEIGHT_PCT: u16 = 10;
const MAX_COMMIT_PANE_HEIGHT_PCT: u16 = 80;

fn pct_from_columns(columns: u16, total_width: u16) -> u16 {
    if total_width == 0 {
        return 0;
    }

    (((u32::from(columns) * 100) + (u32::from(total_width) / 2)) / u32::from(total_width)) as u16
}

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        // Prompts take precedence over every other input target so modal text
        // fields can safely capture keystrokes even when other modes were
        // previously active.
        if !matches!(self.prompt, PromptState::None) {
            return self.handle_prompt_key(key);
        }
        // Macro bar consumes all keys when open.
        if self.macro_bar.is_some() {
            return self.handle_macro_bar_key(key);
        }
        // Interactive mode is handled at the event-loop level via raw stdin
        // passthrough (poll_and_forward_raw_input). When the input target is
        // Agent or Terminal, crossterm's event reader is not called, so
        // handle_key is never reached for those modes.
        debug_assert!(
            !matches!(
                self.input_target,
                InputTarget::Agent | InputTarget::Terminal
            ),
            "handle_key should not be called in interactive mode"
        );
        if self.bindings.lookup(&key, BindingScope::Global) == Some(Action::CloseOverlay)
            && self.close_top_overlay()
        {
            return Ok(false);
        }
        if let Some(ref mut scroll) = self.help_scroll {
            // Help overlay is open — consume all keys, only scroll keys do anything.
            let max_help = self
                .last_help_lines
                .saturating_sub(self.last_help_height.max(1));
            if let Some(action) = self.bindings.lookup(&key, BindingScope::Help) {
                match action {
                    Action::MoveDown => *scroll = (*scroll + 1).min(max_help),
                    Action::MoveUp => *scroll = scroll.saturating_sub(1),
                    Action::ScrollPageDown => {
                        let page = self.last_help_height.max(1);
                        *scroll = (*scroll + page).min(max_help);
                    }
                    Action::ScrollPageUp => {
                        let page = self.last_help_height.max(1);
                        *scroll = scroll.saturating_sub(page);
                    }
                    _ => {}
                }
            } else if key.code == KeyCode::Char(' ') {
                // Space scrolls down one line in the help overlay (not bound
                // via MoveDown to avoid conflicting with ToggleProject/StageUnstage
                // in other scopes).
                *scroll = (*scroll + 1).min(max_help);
            }
            return Ok(false);
        }
        // When typing a commit message, route all keys to the commit input
        // handler so that q, ?, [ etc. are typed instead of triggering
        // global shortcuts.
        if self.input_target == InputTarget::CommitMessage {
            self.handle_commit_input_key(key)?;
            return Ok(false);
        }
        // Check if a Global binding should defer to a pane-scoped binding.
        // For example, `q` is Quit globally but ScrollToBottom in Center — if
        // the focused pane has a binding for this key, skip the global handler
        // and let the pane handler run instead.
        let defer_global = if self.bindings.lookup(&key, BindingScope::Global) == Some(Action::Quit)
        {
            let pane_scope = match self.focus {
                FocusPane::Center => Some(BindingScope::Center),
                FocusPane::Files => Some(BindingScope::Files),
                _ => None,
            };
            pane_scope.is_some_and(|scope| self.bindings.lookup(&key, scope).is_some())
        } else {
            false
        };

        if !defer_global && let Some(action) = self.bindings.lookup(&key, BindingScope::Global) {
            match action {
                Action::Quit => {
                    let agent_count = self.providers.len();
                    let terminal_count = self.running_companion_terminal_count();
                    if agent_count + terminal_count > 0 {
                        self.prompt = PromptState::ConfirmQuit {
                            agent_count,
                            terminal_count,
                            confirm_selected: false,
                        };
                        return Ok(false);
                    }
                    return Ok(true);
                }
                Action::ToggleHelp => {
                    self.help_scroll = if self.help_scroll.is_some() {
                        None
                    } else {
                        Some(0)
                    };
                }
                Action::ForceRedraw => {
                    self.force_redraw = true;
                    self.set_info("Interface redrawn. All screen contents have been repainted.");
                }
                Action::OpenPalette => {
                    self.prompt = PromptState::Command {
                        input: TextInput::new(),
                        selected: 0,
                    };
                    self.set_info("Command palette opened.");
                }
                Action::FocusNext => {
                    let has_staged = !self.staged_files.is_empty();
                    if self.focus == FocusPane::Left
                        && self.left_section == LeftSection::Projects
                        && self.has_terminal_items()
                    {
                        self.left_section = LeftSection::Terminals;
                        self.clamp_terminal_cursor();
                    } else if self.focus == FocusPane::Left
                        && self.left_section == LeftSection::Terminals
                    {
                        self.left_section = LeftSection::Projects;
                        self.focus = self.focus.next();
                    } else if self.focus == FocusPane::Files {
                        match self.right_section.next(has_staged) {
                            Some(next) => {
                                self.right_section = next;
                                self.clamp_files_cursor();
                            }
                            None => {
                                self.focus = self.focus.next();
                                self.left_section = LeftSection::Projects;
                            }
                        }
                    } else {
                        self.focus = self.focus.next();
                        if self.focus == FocusPane::Files && self.right_hidden {
                            self.focus = self.focus.next();
                        }
                        if self.focus == FocusPane::Files {
                            self.right_section = RightSection::first();
                            self.clamp_files_cursor();
                        } else if self.focus == FocusPane::Left {
                            self.left_section = LeftSection::Projects;
                        }
                    }
                    self.input_target = InputTarget::None;
                    self.fullscreen_overlay = FullscreenOverlay::None;
                }
                Action::FocusPrev => {
                    let has_staged = !self.staged_files.is_empty();
                    if self.focus == FocusPane::Left && self.left_section == LeftSection::Terminals
                    {
                        self.left_section = LeftSection::Projects;
                    } else if self.focus == FocusPane::Left
                        && self.left_section == LeftSection::Projects
                    {
                        self.focus = self.focus.previous();
                        if self.focus == FocusPane::Files && self.right_hidden {
                            self.focus = self.focus.previous();
                        }
                        if self.focus == FocusPane::Files {
                            self.right_section = RightSection::last(has_staged);
                            self.clamp_files_cursor();
                        }
                    } else if self.focus == FocusPane::Files {
                        match self.right_section.previous() {
                            Some(prev) => {
                                self.right_section = prev;
                                self.clamp_files_cursor();
                            }
                            None => {
                                self.focus = self.focus.previous();
                            }
                        }
                    } else {
                        self.focus = self.focus.previous();
                        if self.focus == FocusPane::Files && self.right_hidden {
                            self.focus = self.focus.previous();
                        }
                        if self.focus == FocusPane::Files {
                            self.right_section = RightSection::last(has_staged);
                            self.clamp_files_cursor();
                        } else if self.focus == FocusPane::Left {
                            if self.has_terminal_items() {
                                self.left_section = LeftSection::Terminals;
                                self.clamp_terminal_cursor();
                            } else {
                                self.left_section = LeftSection::Projects;
                            }
                        }
                    }
                    self.input_target = InputTarget::None;
                    self.fullscreen_overlay = FullscreenOverlay::None;
                }
                Action::ToggleSidebar => {
                    self.left_collapsed = !self.left_collapsed;
                }
                Action::ToggleGitPane => {
                    self.right_collapsed = !self.right_collapsed;
                    if self.right_collapsed && self.focus == FocusPane::Files {
                        self.focus = FocusPane::Center;
                    }
                }
                Action::RemoveGitPane => {
                    self.right_hidden = !self.right_hidden;
                    if self.right_hidden && self.focus == FocusPane::Files {
                        self.focus = FocusPane::Center;
                    }
                }
                Action::ToggleResizeMode => {
                    self.resize_mode = !self.resize_mode;
                    if self.resize_mode {
                        let grow = self.bindings.labels_for(Action::ResizeGrow);
                        let shrink = self.bindings.labels_for(Action::ResizeShrink);
                        self.set_info(format!(
                            "Resize mode on: {shrink}/{grow} resize side panes."
                        ));
                    } else {
                        self.persist_pane_widths();
                        let key = self.bindings.label_for(Action::ToggleResizeMode);
                        self.set_info(format!(
                            "Resize mode off. Pane widths saved. Press {key} to re-enter."
                        ));
                    }
                }
                _ => {}
            }
            return Ok(false);
        } // !defer_global
        if self.resize_mode {
            self.handle_resize_key(key);
            return Ok(false);
        }

        match self.focus {
            FocusPane::Left => self.handle_left_key(key)?,
            FocusPane::Center => self.handle_center_key(key)?,
            FocusPane::Files => self.handle_files_key(key)?,
        }
        Ok(false)
    }

    fn handle_left_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.left_section == LeftSection::Terminals {
            return self.handle_left_terminal_key(key);
        }

        let item_count = self.left_items().len();
        if let Some(action) = self.bindings.lookup(&key, BindingScope::Left) {
            match action {
                Action::MoveDown => {
                    if self.selected_left + 1 < item_count {
                        self.selected_left += 1;
                        self.reload_changed_files();
                    } else if self.has_terminal_items() {
                        // Jump to terminals section.
                        self.left_section = LeftSection::Terminals;
                        self.selected_terminal_index = 0;
                    }
                }
                Action::MoveUp => {
                    if self.selected_left > 0 {
                        self.selected_left -= 1;
                        self.reload_changed_files();
                    }
                }
                Action::FocusAgent | Action::ExitInteractive => {
                    self.activate_selected_left_item()?
                }
                Action::OpenProjectBrowser => {
                    self.open_project_browser()?;
                }
                Action::NewAgent => self.create_agent_for_selected_project()?,
                Action::ForkAgent => self.fork_selected_session()?,
                Action::RefreshProject => self.refresh_selected_project()?,
                Action::ShowTerminal => self.show_or_open_first_terminal()?,
                Action::DeleteSession => self.confirm_delete_selected_session()?,
                Action::RenameSession => self.open_rename_session()?,
                Action::EditMacros => self.open_edit_macros(),
                Action::CycleProvider => self.cycle_selected_project_provider()?,
                Action::CopyPath => self.copy_selected_path()?,
                Action::OpenWorktreeInEditor => self.open_selected_worktree_in_default_editor()?,
                Action::ChooseWorktreeEditor => self.open_worktree_editor_picker()?,
                Action::ToggleProject => self.toggle_collapse_selected_project(),
                Action::InteractAgent => {
                    if self.selected_session().is_some()
                        && self
                            .selected_session()
                            .map(|s| self.providers.contains_key(&s.id))
                            .unwrap_or(false)
                    {
                        self.focus = FocusPane::Center;
                        self.center_mode = CenterMode::Agent;
                        self.input_target = InputTarget::Agent;
                        self.fullscreen_overlay = FullscreenOverlay::Agent;
                        let exit_key = self.bindings.label_for(Action::ExitInteractive);
                        self.set_info(format!(
                            "Interactive mode. Keys forwarded to agent. {exit_key} exits."
                        ));
                    } else {
                        let r = self.bindings.label_for(Action::ReconnectAgent);
                        let n = self.bindings.label_for(Action::NewAgent);
                        self.set_error(
                            format!("No active agent. Press \"{r}\" to restart or \"{n}\" to create a new one."),
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_left_terminal_key(&mut self, key: KeyEvent) -> Result<()> {
        let term_count = self.terminal_items().len();
        if let Some(action) = self.bindings.lookup(&key, BindingScope::Left) {
            match action {
                Action::MoveDown => {
                    if self.selected_terminal_index + 1 < term_count {
                        self.selected_terminal_index += 1;
                    }
                }
                Action::MoveUp => {
                    if self.selected_terminal_index > 0 {
                        self.selected_terminal_index -= 1;
                    } else {
                        // Jump back to projects section.
                        self.left_section = LeftSection::Projects;
                        let item_count = self.left_items().len();
                        if item_count > 0 {
                            self.selected_left = item_count - 1;
                        }
                    }
                }
                Action::FocusAgent | Action::ExitInteractive => {
                    // Open terminal overlay for the selected terminal item.
                    self.open_terminal_from_terminal_list()?;
                }
                Action::ShowTerminal => {
                    self.spawn_terminal_for_selected_terminal()?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_center_key(&mut self, key: KeyEvent) -> Result<()> {
        let in_diff = matches!(self.center_mode, CenterMode::Diff { .. });
        if let Some(action) = self.bindings.lookup(&key, BindingScope::Center) {
            match action {
                Action::FocusAgent if !in_diff => self.activate_center_agent()?,
                Action::ExitInteractive if !in_diff => self.activate_center_agent()?,
                Action::ShowTerminal if !in_diff => self.show_or_open_first_terminal()?,
                Action::DeleteSession if !in_diff => self.confirm_delete_selected_session()?,
                Action::RenameSession if !in_diff => self.open_rename_session()?,
                Action::ReconnectAgent if !in_diff => {
                    // Allow relaunching an exited agent from the center pane,
                    // or entering interactive mode if the agent is active.
                    let has_provider = self
                        .selected_session()
                        .map(|s| self.providers.contains_key(&s.id))
                        .unwrap_or(false);
                    if has_provider {
                        self.reset_pty_scrollback();
                        self.input_target = InputTarget::Agent;
                        self.fullscreen_overlay = FullscreenOverlay::Agent;
                    } else if self.selected_session().is_some() {
                        self.reconnect_selected_session()?;
                    }
                }
                Action::ScrollPageUp => {
                    if let CenterMode::Diff { ref mut scroll, .. } = self.center_mode {
                        let page = self.last_diff_height.max(1);
                        *scroll = scroll.saturating_sub(page);
                    } else if self.last_pty_size.0 > 0 {
                        self.scroll_pty(ScrollDirection::Up, self.last_pty_size.0 as usize);
                    }
                }
                Action::ScrollPageDown => {
                    if let CenterMode::Diff { ref mut scroll, .. } = self.center_mode {
                        let page = self.last_diff_height.max(1);
                        let max_scroll = self
                            .last_diff_visual_lines
                            .saturating_sub(self.last_diff_height.max(1));
                        *scroll = (*scroll + page).min(max_scroll);
                    } else if self.last_pty_size.0 > 0 {
                        self.scroll_pty(ScrollDirection::Down, self.last_pty_size.0 as usize);
                    }
                }
                Action::ScrollLineUp => {
                    if let CenterMode::Diff { ref mut scroll, .. } = self.center_mode {
                        *scroll = scroll.saturating_sub(1);
                    } else if self.last_pty_size.0 > 0 {
                        self.scroll_pty(ScrollDirection::Up, 1);
                    }
                }
                Action::ScrollLineDown => {
                    if let CenterMode::Diff { ref mut scroll, .. } = self.center_mode {
                        let max_scroll = self
                            .last_diff_visual_lines
                            .saturating_sub(self.last_diff_height.max(1));
                        *scroll = (*scroll + 1).min(max_scroll);
                    } else if self.last_pty_size.0 > 0 {
                        self.scroll_pty(ScrollDirection::Down, 1);
                    }
                }
                Action::ScrollToBottom => {
                    if let CenterMode::Diff { ref mut scroll, .. } = self.center_mode {
                        let max_scroll = self
                            .last_diff_visual_lines
                            .saturating_sub(self.last_diff_height.max(1));
                        *scroll = max_scroll;
                    } else {
                        self.reset_pty_scrollback();
                    }
                }
                _ => {}
            }
        } else if !in_diff && self.input_target == InputTarget::None {
            let is_typeable = matches!(
                key.code,
                KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace
            );
            if is_typeable && key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
                self.readonly_nudge_tick = Some(self.tick_count);
            }
        }
        Ok(())
    }

    fn scroll_pty(&mut self, direction: ScrollDirection, amount: usize) {
        let provider = match self.selected_terminal_surface_client() {
            Some(provider) => provider,
            None => return,
        };
        let up = matches!(direction, ScrollDirection::Up);
        provider.scroll(up, amount);
    }

    fn reset_pty_scrollback(&self) {
        if let Some(provider) = self.selected_terminal_surface_client() {
            provider.set_scrollback(0);
        }
    }

    fn handle_files_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.files_search_active = false;
                return;
            }
            _ => {}
        }
        if self.files_search.handle_key(key) {
            let query = self.files_search.text.clone();
            let found_match = self.update_files_search(query);
            if !found_match && self.has_files_search() {
                self.set_info("No file matches the current search.");
            }
        }
    }

    fn handle_files_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.files_search_active {
            self.handle_files_search_key(key);
            return Ok(());
        }

        if let Some(action) = self.bindings.lookup(&key, BindingScope::Files) {
            match action {
                Action::MoveDown => {
                    if self.right_section != RightSection::CommitInput {
                        let len = self.current_files_len();
                        if self.files_index + 1 < len {
                            self.files_index += 1;
                        }
                    }
                }
                Action::MoveUp => {
                    if self.right_section != RightSection::CommitInput && self.files_index > 0 {
                        self.files_index -= 1;
                    }
                }
                Action::StageUnstage => {
                    if self.right_section != RightSection::CommitInput {
                        self.toggle_stage_selected_file()?;
                    }
                }
                Action::CommitChanges if !self.staged_files.is_empty() => {
                    self.execute_commit()?;
                }
                Action::OpenDiff => {
                    if self.right_section == RightSection::CommitInput {
                        if !self.staged_files.is_empty() {
                            self.input_target = InputTarget::CommitMessage;
                        }
                    } else {
                        self.open_diff_for_selected_file()?;
                    }
                }
                Action::DiscardChanges => {
                    if self.right_section != RightSection::CommitInput {
                        self.confirm_discard_selected_file()?;
                    }
                }
                Action::GenerateCommitMessage => {
                    self.trigger_ai_commit_message()?;
                }
                Action::EngageCommitInput if !self.staged_files.is_empty() => {
                    self.input_target = InputTarget::CommitMessage;
                }
                Action::PushToRemote => {
                    self.push_to_remote()?;
                }
                Action::PullFromRemote => {
                    self.pull_from_remote()?;
                }
                Action::SearchFiles => {
                    self.files_search_active = true;
                }
                Action::SearchNext => {
                    if !self.advance_files_search_match() {
                        self.set_info("No active file search matches.");
                    }
                }
                _ => {}
            }
        } else if key.code == KeyCode::Esc && self.has_files_search() {
            self.clear_files_search();
        }
        Ok(())
    }

    fn handle_commit_input_key(&mut self, key: KeyEvent) -> Result<()> {
        // Exit actions are dispatched via bindings; text input stays hardcoded.
        if let Some(Action::ExitCommitInput) = self.bindings.lookup(&key, BindingScope::CommitInput)
        {
            self.input_target = InputTarget::None;
            return Ok(());
        }
        // TextInput handles Enter (newline), Up/Down (line nav), and all
        // editing keys in multiline mode.
        self.commit_input.handle_key(key);
        Ok(())
    }

    fn handle_macro_bar_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.close_macro_bar();
            }
            KeyCode::Enter => {
                let (query, selected) = if let Some(bar) = &self.macro_bar {
                    (bar.input.text.clone(), bar.selected)
                } else {
                    return Ok(false);
                };
                let filtered = self.filtered_macros(&query);
                if let Some(&(name, text)) = filtered.get(selected) {
                    if let Some(provider) = self.selected_terminal_surface_client() {
                        let mut payload = Vec::with_capacity(text.len() + 12);
                        payload.extend_from_slice(b"\x1b[200~");
                        payload.extend_from_slice(text.as_bytes());
                        payload.extend_from_slice(b"\x1b[201~");
                        let _ = provider.write_bytes(&payload);
                    }
                    let name = name.to_string();
                    self.set_info(format!("Pasted macro \"{name}\"."));
                }
                self.close_macro_bar();
            }
            KeyCode::Up => {
                if let Some(bar) = &mut self.macro_bar {
                    bar.selected = bar.selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                let count = if let Some(bar) = &self.macro_bar {
                    let query = bar.input.text.clone();
                    self.filtered_macros(&query).len()
                } else {
                    0
                };
                if let Some(bar) = &mut self.macro_bar {
                    bar.selected = (bar.selected + 1).min(count.saturating_sub(1));
                }
            }
            KeyCode::Tab => {
                if let Some(bar) = &self.macro_bar {
                    let query = bar.input.text.clone();
                    let selected = bar.selected;
                    let filtered = self.filtered_macros(&query);
                    if let Some(&(name, _)) = filtered.get(selected) {
                        let name = name.to_string();
                        if let Some(bar) = &mut self.macro_bar {
                            bar.input.set_text(name);
                            bar.selected = 0;
                        }
                    }
                }
            }
            _ => {
                if let Some(bar) = &mut self.macro_bar {
                    let changed = bar.input.handle_key(key);
                    if changed {
                        bar.selected = 0;
                    }
                }
            }
        }
        Ok(false)
    }

    fn close_macro_bar(&mut self) {
        if let Some(bar) = self.macro_bar.take() {
            self.input_target = bar.previous_input_target;
        }
    }

    fn toggle_stage_selected_file(&mut self) -> Result<()> {
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let worktree = PathBuf::from(&session.worktree_path);
        let file = match self.right_section {
            RightSection::Staged => self.staged_files.get(self.files_index),
            RightSection::Unstaged => self.unstaged_files.get(self.files_index),
            RightSection::CommitInput => return Ok(()),
        };
        let Some(file) = file else { return Ok(()) };
        let path = file.path.clone();
        match self.right_section {
            RightSection::Unstaged => {
                git::stage_file(&worktree, &path)?;
            }
            RightSection::Staged => {
                git::unstage_file(&worktree, &path)?;
            }
            RightSection::CommitInput => {}
        }
        self.reload_changed_files();
        // If the section we were in is now empty, move to the other one.
        if self.right_section == RightSection::Staged && self.staged_files.is_empty() {
            self.right_section = RightSection::Unstaged;
            self.clamp_files_cursor();
        } else if self.right_section == RightSection::Unstaged && self.unstaged_files.is_empty() {
            self.right_section = RightSection::Staged;
            self.clamp_files_cursor();
        }
        Ok(())
    }

    fn confirm_discard_selected_file(&mut self) -> Result<()> {
        let file = match self.right_section {
            RightSection::Unstaged => self.unstaged_files.get(self.files_index),
            RightSection::Staged => {
                self.set_error("Unstage the file first to discard changes.");
                return Ok(());
            }
            RightSection::CommitInput => return Ok(()),
        };
        let Some(file) = file else { return Ok(()) };
        self.prompt = PromptState::ConfirmDiscardFile {
            file_path: file.path.clone(),
            is_untracked: file.status == "?",
            confirm_selected: false,
        };
        Ok(())
    }

    fn trigger_ai_commit_message(&mut self) -> Result<()> {
        if self.staged_files.is_empty() {
            self.set_error("Stage files first.");
            return Ok(());
        }
        if self.commit_input.overlay().is_some() {
            return Ok(());
        }
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let worktree = PathBuf::from(&session.worktree_path);
        let project_path = session.project_path.as_deref().unwrap_or("");
        let base_prompt = self.config.commit_prompt_for_project(project_path);

        // Capture the staged diff up-front so the provider does not need tool
        // access to inspect it. The diff is appended after the prompt text.
        let diff_text = match git::staged_diff_text(&worktree) {
            Ok(d) if d.is_empty() => {
                self.set_error("No staged diff found.");
                return Ok(());
            }
            Ok(d) => d,
            Err(e) => {
                self.set_error(format!("Failed to read staged diff: {e}"));
                return Ok(());
            }
        };
        let prompt = format!("{base_prompt}\n\n{diff_text}");

        let cfg = provider_config(&self.config, &session.provider);
        let prov = provider::create_provider(session.provider.as_str(), cfg);
        let tx = self.worker_tx.clone();
        self.commit_input
            .set_overlay("Generating commit message\u{2026}");
        self.set_busy("Generating AI commit message from staged diff\u{2026}");
        thread::spawn(move || match prov.run_oneshot(&prompt, &worktree) {
            Ok(msg) => {
                let _ = tx.send(WorkerEvent::CommitMessageGenerated(msg));
            }
            Err(e) => {
                let _ = tx.send(WorkerEvent::CommitMessageFailed(e.to_string()));
            }
        });
        Ok(())
    }

    fn execute_commit(&mut self) -> Result<()> {
        if self.staged_files.is_empty() {
            self.set_error("No staged changes to commit.");
            return Ok(());
        }
        if self.commit_input.text.trim().is_empty() {
            self.set_error("Enter a commit message first.");
            return Ok(());
        }
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let worktree = PathBuf::from(&session.worktree_path);
        match git::commit(&worktree, &self.commit_input.text) {
            Ok(_) => {
                self.commit_input.clear();
                let push_key = self.bindings.label_for(Action::PushToRemote);
                let ai_key = self.bindings.label_for(Action::GenerateCommitMessage);
                self.set_info(format!("Changes committed successfully. Press {push_key} to push to remote, or {ai_key} to generate an AI message."));
                self.reload_changed_files();
            }
            Err(e) => self.set_error(format!("Commit failed: {e}")),
        }
        Ok(())
    }

    fn push_to_remote(&mut self) -> Result<()> {
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let worktree = PathBuf::from(&session.worktree_path);
        let tx = self.worker_tx.clone();
        self.set_busy("Pushing to remote…");
        thread::spawn(move || {
            let result = git::push(&worktree).map(|_| ()).map_err(|e| e.to_string());
            let _ = tx.send(WorkerEvent::PushCompleted(result));
        });
        Ok(())
    }

    pub(crate) fn start_pull(
        &mut self,
        repo_path: PathBuf,
        target: PullTarget,
        busy_message: impl Into<String>,
        already_running_message: impl Into<String>,
    ) {
        let repo_key = repo_path.to_string_lossy().into_owned();
        if !self.pulls_in_flight.insert(repo_key.clone()) {
            self.set_warning(already_running_message);
            return;
        }

        let tx = self.worker_tx.clone();
        self.set_busy(busy_message);
        thread::spawn(move || {
            let result = match &target {
                PullTarget::Project { .. } => match git::is_dirty(&repo_path) {
                    Ok(true) => Err(
                        "Refresh blocked because the source checkout has uncommitted changes."
                            .to_string(),
                    ),
                    Ok(false) => git::pull_current_branch(&repo_path)
                        .map(|_| git::current_branch(&repo_path).ok())
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                },
                PullTarget::Session => git::pull_current_branch(&repo_path)
                    .map(|_| None)
                    .map_err(|e| e.to_string()),
            };
            let _ = tx.send(WorkerEvent::PullCompleted {
                repo_path: repo_key,
                target,
                result,
            });
        });
    }

    fn pull_from_remote(&mut self) -> Result<()> {
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        self.start_pull(
            PathBuf::from(&session.worktree_path),
            PullTarget::Session,
            "Pulling latest changes from remote…",
            "Pull already in progress for this worktree. Wait for the current pull to finish.",
        );
        Ok(())
    }

    /// Read raw bytes from stdin, split into terminal sequences, and either
    /// handle intercepted bindings (exit interactive, scroll) or forward
    /// verbatim to the active PTY. This bypasses crossterm's event parser so
    /// all key combinations (Shift+Tab, Alt+Backspace, Ctrl+Right, F-keys,
    /// kitty keyboard protocol sequences, etc.) reach the child process.
    pub(crate) fn poll_and_forward_raw_input(&mut self) -> Result<bool> {
        use rustix::event::{PollFd, PollFlags, poll};
        use std::io::Read;
        use std::os::fd::AsFd;

        // Verify we have an active session/provider.
        if self.selected_session().is_none() {
            self.input_target = InputTarget::None;
            self.terminal_selection = None;
            self.raw_input_buf.clear();
            return Ok(false);
        }
        let surface = self.session_surface;
        if self.selected_terminal_surface_client().is_none() {
            self.input_target = InputTarget::None;
            self.terminal_selection = None;
            self.raw_input_buf.clear();
            let label = match surface {
                SessionSurface::Agent => "Agent",
                SessionSurface::Terminal => "Companion terminal",
            };
            self.set_error(format!("{label} disconnected."));
            return Ok(false);
        }

        // Poll stdin with 100ms timeout (matches the crossterm poll interval).
        let stdin_handle = std::io::stdin();
        let timeout = rustix::time::Timespec {
            tv_sec: 0,
            tv_nsec: 100_000_000, // 100ms
        };
        let stdin_borrow = stdin_handle.as_fd();
        let ready = crate::io_retry::retry_on_interrupt_errno(|| {
            let mut pollfd = [PollFd::new(&stdin_borrow, PollFlags::IN)];
            poll(&mut pollfd, Some(&timeout))
        })?;
        if ready == 0 {
            return Ok(false);
        }

        // Read available bytes from the same handle used for polling.
        let mut buf = [0u8; 4096];
        let mut stdin_lock = stdin_handle.lock();
        let n = crate::io_retry::retry_on_interrupt(|| stdin_lock.read(&mut buf))?;
        if n == 0 {
            return Ok(false);
        }

        // Don't forward input until the agent has produced visible output.
        // Keystrokes during the loading phase would reach a process that
        // isn't ready for them. We still drain stdin above to prevent
        // buffer accumulation.
        if let Some(provider) = self.selected_terminal_surface_client()
            && !provider.has_output()
        {
            return Ok(false);
        }

        self.process_raw_input_bytes(&buf[..n])
    }

    /// Process raw bytes that have already been read from stdin.
    ///
    /// This is the core logic of interactive input handling, split out from
    /// `poll_and_forward_raw_input` so it can be tested without real stdin I/O.
    pub(crate) fn process_raw_input_bytes(&mut self, bytes: &[u8]) -> Result<bool> {
        self.raw_input_buf.extend_from_slice(bytes);

        // Split into complete sequences and collect actions to avoid borrow
        // conflicts between raw_input_buf and &mut self methods.
        let (sequences, remainder) = crate::raw_input::split_sequences(&self.raw_input_buf);
        let remainder_len = remainder.len();

        // Collect what to do for each sequence: an intercepted action, a
        // mouse event to handle in the UI, or raw bytes to forward.
        enum SeqAction {
            Intercept(Action, bool, Vec<u8>),
            Mouse(MouseEvent, Vec<u8>),
            Forward(Vec<u8>),
        }

        let actions: Vec<SeqAction> = sequences
            .iter()
            .map(|seq| {
                // Mouse events must be handled by the UI, not forwarded to the
                // PTY. crossterm's EnableMouseCapture uses SGR (1006) encoding,
                // so terminal mouse events arrive as CSI `<…M` / `<…m`.
                if let Some(mouse_ev) = crate::raw_input::parse_sgr_mouse(seq) {
                    return SeqAction::Mouse(mouse_ev, seq.to_vec());
                }
                if let Some((action, conditional)) = self.interactive_patterns.match_sequence(seq) {
                    SeqAction::Intercept(action, conditional, seq.to_vec())
                } else {
                    SeqAction::Forward(seq.to_vec())
                }
            })
            .collect();

        // Trim buffer to remainder.
        let buf_len = self.raw_input_buf.len();
        let keep_from = buf_len - remainder_len;
        self.raw_input_buf = self.raw_input_buf[keep_from..].to_vec();

        // Check once whether the user is scrolled back so we can suppress
        // non-scroll input for the entire batch.
        let is_scrolled_back = self
            .selected_terminal_surface_client()
            .is_some_and(|p| p.scrollback_offset() > 0);

        // Process collected actions.
        for action in actions {
            match action {
                SeqAction::Intercept(Action::OpenMacroBar, _, _) => {
                    if is_scrolled_back {
                        continue;
                    }
                    if self.filtered_macros("").is_empty() {
                        self.set_info("No macros defined for this surface.");
                        self.raw_input_buf.clear();
                        return Ok(false);
                    }
                    let prev = self.input_target;
                    self.macro_bar = Some(MacroBarState {
                        input: TextInput::new(),
                        selected: 0,
                        previous_input_target: prev,
                    });
                    self.input_target = InputTarget::None;
                    self.terminal_selection = None;
                    self.raw_input_buf.clear();
                    return Ok(false);
                }
                SeqAction::Intercept(Action::ExitInteractive, _, _) => {
                    let return_to_terminal_list =
                        matches!(self.input_target, InputTarget::Terminal)
                            && self.terminal_return_to_list;
                    self.input_target = InputTarget::None;
                    self.fullscreen_overlay = FullscreenOverlay::None;
                    self.session_surface = SessionSurface::Agent;
                    self.terminal_selection = None;
                    self.raw_input_buf.clear();
                    if return_to_terminal_list {
                        self.left_section = LeftSection::Terminals;
                        self.clamp_terminal_cursor();
                        self.focus = FocusPane::Left;
                    }
                    self.set_info("Exited interactive mode.");
                    return Ok(false);
                }
                SeqAction::Intercept(Action::ScrollPageUp, _, _) => {
                    if self.last_pty_size.0 > 0 {
                        self.scroll_pty(ScrollDirection::Up, self.last_pty_size.0 as usize);
                    }
                }
                SeqAction::Intercept(Action::ScrollPageDown, _, _) => {
                    if self.last_pty_size.0 > 0 {
                        self.scroll_pty(ScrollDirection::Down, self.last_pty_size.0 as usize);
                    }
                }
                SeqAction::Intercept(Action::ScrollLineUp, conditional, raw) => {
                    let has_scrollback = self
                        .selected_terminal_surface_client()
                        .is_some_and(|p| p.scrollback_offset() > 0);
                    if conditional && has_scrollback && self.last_pty_size.0 > 0 {
                        self.scroll_pty(ScrollDirection::Up, 1);
                    } else if let Some(provider) = self.selected_terminal_surface_client() {
                        let _ = provider.write_bytes(&raw);
                    }
                }
                SeqAction::Intercept(Action::ScrollLineDown, conditional, raw) => {
                    let has_scrollback = self
                        .selected_terminal_surface_client()
                        .is_some_and(|p| p.scrollback_offset() > 0);
                    if conditional && has_scrollback && self.last_pty_size.0 > 0 {
                        self.scroll_pty(ScrollDirection::Down, 1);
                    } else if let Some(provider) = self.selected_terminal_surface_client() {
                        let _ = provider.write_bytes(&raw);
                    }
                }
                SeqAction::Intercept(Action::ScrollToBottom, conditional, raw) => {
                    let has_scrollback = self
                        .selected_terminal_surface_client()
                        .is_some_and(|p| p.scrollback_offset() > 0);
                    if conditional && has_scrollback {
                        self.reset_pty_scrollback();
                    } else if let Some(provider) = self.selected_terminal_surface_client() {
                        let _ = provider.write_bytes(&raw);
                    }
                }
                SeqAction::Mouse(mouse_ev, raw) => {
                    let is_scroll = matches!(
                        mouse_ev.kind,
                        MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight
                    );
                    if is_scroll {
                        self.terminal_selection = None;
                        // Check if the provider has forward_scroll enabled
                        // (only applies to agents, not companion terminals).
                        let forward = matches!(self.input_target, InputTarget::Agent)
                            && self
                                .selected_session()
                                .map(|s| provider_config(&self.config, &s.provider).forward_scroll)
                                .unwrap_or(false);
                        if forward {
                            if let Some(provider) = self.selected_terminal_surface_client() {
                                let _ = provider.write_bytes(&raw);
                            }
                        } else if self.handle_mouse(mouse_ev) {
                            return Ok(true);
                        }
                    } else {
                        // If the click landed outside the fullscreen overlay,
                        // exit interactive mode instead of forwarding to the PTY.
                        // This check runs regardless of scroll state so the user
                        // can always click outside to dismiss the overlay.
                        let outside_overlay =
                            matches!(mouse_ev.kind, MouseEventKind::Down(MouseButton::Left))
                                && !self.mouse_layout.agent_term.is_some_and(|rect| {
                                    contains_point(rect, mouse_ev.column, mouse_ev.row)
                                });
                        if outside_overlay {
                            let return_to_terminal_list =
                                matches!(self.input_target, InputTarget::Terminal)
                                    && self.terminal_return_to_list;
                            self.input_target = InputTarget::None;
                            self.fullscreen_overlay = FullscreenOverlay::None;
                            self.session_surface = SessionSurface::Agent;
                            self.raw_input_buf.clear();
                            self.terminal_selection = None;
                            if return_to_terminal_list {
                                self.left_section = LeftSection::Terminals;
                                self.clamp_terminal_cursor();
                                self.focus = FocusPane::Left;
                            }
                            self.set_info("Exited interactive mode.");
                            return Ok(false);
                        }

                        let child_wants_mouse = self
                            .selected_terminal_surface_client()
                            .is_some_and(|p| p.has_mouse_mode());
                        let shift_held = mouse_ev
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::SHIFT);
                        let should_select = !child_wants_mouse || shift_held;

                        if should_select {
                            self.handle_terminal_selection_mouse(mouse_ev);
                        } else if child_wants_mouse
                            && !is_scrolled_back
                            && let Some(provider) = self.selected_terminal_surface_client()
                        {
                            let _ = provider.write_bytes(&raw);
                        }
                    }
                }
                SeqAction::Intercept(_, _, raw) | SeqAction::Forward(raw) => {
                    // Unknown intercepted action or normal forward — send to PTY,
                    // but only when not scrolled back. In scroll mode, all
                    // non-scroll input is suppressed.
                    self.terminal_selection = None;
                    if !is_scrolled_back
                        && let Some(provider) = self.selected_terminal_surface_client()
                    {
                        let _ = provider.write_bytes(&raw);
                    }
                }
            }
        }

        Ok(false)
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        if let PromptState::ResourceMonitor {
            scroll_offset,
            rows,
            ..
        } = &mut self.prompt
        {
            if key.code == KeyCode::Esc {
                self.prompt = PromptState::None;
                return Ok(false);
            }
            let max_offset = rows.len().saturating_sub(1) as u16;
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    *scroll_offset = scroll_offset.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *scroll_offset = (*scroll_offset + 1).min(max_offset);
                }
                KeyCode::PageUp => {
                    *scroll_offset = scroll_offset.saturating_sub(10);
                }
                KeyCode::PageDown => {
                    *scroll_offset = (*scroll_offset + 10).min(max_offset);
                }
                _ => {}
            }
            return Ok(false);
        }

        if let PromptState::DebugInput {
            lines,
            scroll_offset,
        } = &mut self.prompt
        {
            // Esc always closes — hardcoded so a broken binding can't trap the user.
            if key.code == KeyCode::Esc {
                self.prompt = PromptState::None;
                return Ok(false);
            }

            let kc = crokey::KeyCombination::from(key).normalized();
            let label = crate::keybindings::display_format().to_string(kc);

            // Look up what action this key resolves to in every scope.
            let resolved: Vec<String> = BindingScope::ALL
                .iter()
                .filter_map(|&scope| {
                    self.bindings
                        .lookup(&key, scope)
                        .map(|action| format!("{}: {}", scope.display_name(), action.config_name()))
                })
                .collect();
            let action_text = if resolved.is_empty() {
                "(none)".to_string()
            } else {
                resolved.join(", ")
            };

            let ts = Local::now().format("%H:%M:%S%.3f").to_string();
            lines.push(Line::from(vec![
                Span::styled(ts, Style::default().add_modifier(Modifier::DIM)),
                Span::raw(" │ "),
                Span::styled(
                    "Key   ",
                    Style::default().fg(self.theme.help_section_header_fg),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<18}", label),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(action_text, Style::default().add_modifier(Modifier::DIM)),
            ]));

            // Auto-scroll: keep the view pinned to the bottom.
            let total = lines.len() as u16;
            *scroll_offset = total;

            return Ok(false);
        }

        if matches!(self.prompt, PromptState::Command { .. }) {
            // Plain character keys always go to TextInput so j/k etc. can be
            // typed without conflicting with navigation bindings.
            let is_plain_char = matches!(key.code, KeyCode::Char(_))
                && !key.modifiers.contains(KeyModifiers::CONTROL);
            let action = if is_plain_char {
                None
            } else {
                self.bindings.lookup(&key, BindingScope::Palette)
            };

            match action {
                Some(Action::CloseOverlay) => {
                    self.prompt = PromptState::None;
                }
                Some(Action::MoveDown) => {
                    if let PromptState::Command {
                        input, selected, ..
                    } = &mut self.prompt
                    {
                        let count = self.bindings.filtered_palette(&input.text).len();
                        if *selected + 1 < count {
                            *selected += 1;
                        }
                    }
                }
                Some(Action::MoveUp) => {
                    if let PromptState::Command { selected, .. } = &mut self.prompt
                        && *selected > 0
                    {
                        *selected -= 1;
                    }
                }
                Some(Action::Confirm) => {
                    self.execute_selected_command_palette();
                }
                _ => {
                    // Text input fallback: Tab (autocomplete), then delegate to TextInput.
                    if let PromptState::Command {
                        input, selected, ..
                    } = &mut self.prompt
                    {
                        if key.code == KeyCode::Tab {
                            if let Some(binding) =
                                self.bindings.filtered_palette(&input.text).get(*selected)
                            {
                                input.set_text(binding.palette_name.unwrap().to_string());
                                *selected = 0;
                            }
                        } else if input.handle_key(key) {
                            *selected = 0;
                        }
                    }
                }
            }
            return Ok(false);
        }

        if matches!(self.prompt, PromptState::KillRunning(..)) {
            let is_searching = matches!(
                self.prompt,
                PromptState::KillRunning(KillRunningPrompt {
                    searching: true,
                    ..
                })
            );
            let is_plain_char = matches!(key.code, KeyCode::Char(_))
                && !key.modifiers.contains(KeyModifiers::CONTROL);
            let action = if is_searching && is_plain_char {
                None
            } else {
                self.bindings.lookup(&key, BindingScope::RuntimeKill)
            };

            match action {
                Some(Action::CloseOverlay) => {
                    let mut closed = false;
                    if let PromptState::KillRunning(prompt) = &mut self.prompt {
                        if prompt.searching {
                            prompt.searching = false;
                        } else if !prompt.filter.is_empty() {
                            prompt.filter.clear();
                            Self::clamp_kill_running_prompt(prompt);
                        } else {
                            closed = true;
                        }
                    }
                    if closed {
                        self.prompt = PromptState::None;
                        self.set_info("Closed Kill Running. No agents or terminals were killed.");
                    }
                }
                Some(Action::SearchToggle) if !is_searching => {
                    if let PromptState::KillRunning(prompt) = &mut self.prompt {
                        prompt.filter.move_end();
                        prompt.searching = true;
                        prompt.focus = KillRunningFocus::List;
                    }
                }
                Some(Action::MoveDown) => {
                    if let PromptState::KillRunning(prompt) = &mut self.prompt
                        && matches!(prompt.focus, KillRunningFocus::List)
                    {
                        let count = Self::visible_kill_running_indices(prompt).len();
                        if prompt.hovered_visible_index + 1 < count {
                            prompt.hovered_visible_index += 1;
                        }
                    }
                }
                Some(Action::MoveUp) => {
                    if let PromptState::KillRunning(prompt) = &mut self.prompt
                        && matches!(prompt.focus, KillRunningFocus::List)
                        && prompt.hovered_visible_index > 0
                    {
                        prompt.hovered_visible_index -= 1;
                    }
                }
                Some(Action::FocusNext) => {
                    if let PromptState::KillRunning(prompt) = &mut self.prompt {
                        prompt.focus = match prompt.focus {
                            KillRunningFocus::List => {
                                Self::next_kill_running_footer_action(prompt, None, true)
                            }
                            KillRunningFocus::Footer(action) => {
                                Self::next_kill_running_footer_action(prompt, Some(action), true)
                            }
                        };
                    }
                }
                Some(Action::FocusPrev) => {
                    if let PromptState::KillRunning(prompt) = &mut self.prompt {
                        prompt.focus = match prompt.focus {
                            KillRunningFocus::List => {
                                Self::next_kill_running_footer_action(prompt, None, false)
                            }
                            KillRunningFocus::Footer(action) => {
                                Self::next_kill_running_footer_action(prompt, Some(action), false)
                            }
                        };
                    }
                }
                Some(Action::ToggleMarked) => {
                    self.toggle_hovered_kill_running_selection();
                }
                Some(Action::Confirm) => {
                    if is_searching {
                        if let PromptState::KillRunning(prompt) = &mut self.prompt {
                            prompt.searching = false;
                        }
                    } else {
                        let focus = match &self.prompt {
                            PromptState::KillRunning(prompt) => prompt.focus,
                            _ => KillRunningFocus::List,
                        };
                        match focus {
                            KillRunningFocus::List => self.toggle_hovered_kill_running_selection(),
                            KillRunningFocus::Footer(action) => {
                                self.execute_kill_running_footer_action(action)?;
                            }
                        }
                    }
                }
                _ => {
                    if is_searching && let PromptState::KillRunning(prompt) = &mut self.prompt {
                        if prompt.filter.handle_key(key) {
                            prompt.hovered_visible_index = 0;
                        }
                        Self::clamp_kill_running_prompt(prompt);
                    }
                }
            }
            return Ok(false);
        }

        if matches!(self.prompt, PromptState::BrowseProjects { .. }) {
            // Check sub-mode states and do binding lookup before mutable borrow.
            let is_editing_path = matches!(
                self.prompt,
                PromptState::BrowseProjects {
                    editing_path: true,
                    ..
                }
            );
            let is_searching = matches!(
                self.prompt,
                PromptState::BrowseProjects {
                    searching: true,
                    ..
                }
            );
            let is_plain_char = matches!(key.code, KeyCode::Char(_))
                && !key.modifiers.contains(KeyModifiers::CONTROL);

            // Path editor is pure text input — keep hardcoded KeyCode matches.
            if is_editing_path {
                if let PromptState::BrowseProjects {
                    current_dir,
                    entries,
                    loading,
                    selected,
                    filter,
                    editing_path,
                    path_input,
                    tab_completions,
                    tab_index,
                    ..
                } = &mut self.prompt
                {
                    let mut browse_to: Option<PathBuf> = None;
                    let mut error_msg = None;
                    match key.code {
                        KeyCode::Esc => {
                            *editing_path = false;
                            path_input.clear();
                            tab_completions.clear();
                            *tab_index = 0;
                        }
                        KeyCode::Tab | KeyCode::BackTab => {
                            if tab_completions.is_empty() {
                                let input_path = PathBuf::from(path_input.text.as_str());
                                let (search_dir, prefix) =
                                    if input_path.is_dir() && path_input.text.ends_with('/') {
                                        (input_path.clone(), String::new())
                                    } else {
                                        let parent = input_path
                                            .parent()
                                            .unwrap_or_else(|| std::path::Path::new("/"));
                                        let file_name = input_path
                                            .file_name()
                                            .map(|f| f.to_string_lossy().to_string())
                                            .unwrap_or_default();
                                        (parent.to_path_buf(), file_name)
                                    };
                                if let Ok(read) = std::fs::read_dir(&search_dir) {
                                    let prefix_lower = prefix.to_lowercase();
                                    let mut candidates: Vec<String> = read
                                        .filter_map(|e| e.ok())
                                        .filter(|e| {
                                            e.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
                                        })
                                        .filter(|e| {
                                            let name =
                                                e.file_name().to_string_lossy().to_lowercase();
                                            !name.starts_with('.')
                                                && name.starts_with(&prefix_lower)
                                        })
                                        .map(|e| {
                                            let mut full = search_dir
                                                .join(e.file_name())
                                                .to_string_lossy()
                                                .to_string();
                                            full.push('/');
                                            full
                                        })
                                        .collect();
                                    candidates.sort();
                                    *tab_completions = candidates;
                                    *tab_index = 0;
                                }
                            } else if key.code == KeyCode::BackTab {
                                if *tab_index == 0 {
                                    *tab_index = tab_completions.len().saturating_sub(1);
                                } else {
                                    *tab_index -= 1;
                                }
                            } else {
                                *tab_index = (*tab_index + 1) % tab_completions.len();
                            }
                            if let Some(completion) = tab_completions.get(*tab_index) {
                                path_input.set_text(completion.clone());
                            }
                        }
                        KeyCode::Enter => {
                            let new_dir = PathBuf::from(path_input.text.trim());
                            if new_dir.is_dir() {
                                *current_dir = new_dir.clone();
                                entries.clear();
                                *loading = true;
                                *selected = 0;
                                filter.clear();
                                browse_to = Some(new_dir);
                            } else {
                                error_msg =
                                    Some(format!("{} is not a directory.", path_input.text.trim()));
                            }
                            *editing_path = false;
                            path_input.clear();
                            tab_completions.clear();
                            *tab_index = 0;
                        }
                        KeyCode::Up | KeyCode::Down => {
                            tab_completions.clear();
                            *tab_index = 0;
                        }
                        _ => {
                            if path_input.handle_key(key) {
                                tab_completions.clear();
                                *tab_index = 0;
                            }
                        }
                    }
                    if let Some(msg) = error_msg {
                        self.set_error(msg);
                    }
                    if let Some(dir) = browse_to {
                        self.spawn_browser_entries(&dir);
                    }
                }
                return Ok(false);
            }

            // Browser normal/search mode — use binding lookup.
            let action = if is_searching && is_plain_char {
                None
            } else {
                self.bindings.lookup(&key, BindingScope::Browser)
            };

            match action {
                Some(Action::CloseOverlay) => {
                    if let PromptState::BrowseProjects {
                        searching,
                        filter,
                        selected,
                        ..
                    } = &mut self.prompt
                    {
                        if *searching {
                            *searching = false;
                        } else if !filter.is_empty() {
                            filter.clear();
                            *selected = 0;
                        } else {
                            self.prompt = PromptState::None;
                        }
                    }
                }
                Some(Action::SearchToggle) if !is_searching => {
                    if let PromptState::BrowseProjects {
                        filter, searching, ..
                    } = &mut self.prompt
                    {
                        filter.move_end();
                        *searching = true;
                    }
                }
                Some(Action::MoveDown) => {
                    if let PromptState::BrowseProjects {
                        entries,
                        selected,
                        filter,
                        ..
                    } = &mut self.prompt
                    {
                        let filtered_len = if filter.is_empty() {
                            entries.len()
                        } else {
                            let needle = filter.text.to_lowercase();
                            entries
                                .iter()
                                .filter(|e| e.label.to_lowercase().contains(&needle))
                                .count()
                        };
                        if *selected + 1 < filtered_len {
                            *selected += 1;
                        }
                    }
                }
                Some(Action::MoveUp) => {
                    if let PromptState::BrowseProjects { selected, .. } = &mut self.prompt
                        && *selected > 0
                    {
                        *selected -= 1;
                    }
                }
                Some(Action::GoToPath) if !is_searching => {
                    if let PromptState::BrowseProjects {
                        current_dir,
                        editing_path,
                        path_input,
                        ..
                    } = &mut self.prompt
                    {
                        *editing_path = true;
                        let mut p = current_dir.to_string_lossy().to_string();
                        if !p.ends_with('/') {
                            p.push('/');
                        }
                        path_input.set_text(p);
                    }
                }
                Some(Action::Confirm) if is_searching => {
                    if let PromptState::BrowseProjects { searching, .. } = &mut self.prompt {
                        *searching = false;
                    }
                }
                Some(Action::OpenEntry) if !is_searching => {
                    self.open_selected_browser_entry();
                }
                Some(Action::AddCurrentDir) if !is_searching => {
                    if let PromptState::BrowseProjects { current_dir, .. } = &self.prompt {
                        let path = current_dir.to_string_lossy().to_string();
                        self.prompt = PromptState::None;
                        if let Err(e) = self.add_project(path, String::new()) {
                            self.set_error(format!("{e:#}"));
                        }
                    }
                }
                _ => {
                    // Text input fallback for search mode.
                    if is_searching
                        && let PromptState::BrowseProjects {
                            filter, selected, ..
                        } = &mut self.prompt
                        && filter.handle_key(key)
                    {
                        *selected = 0;
                    }
                }
            }
            return Ok(false);
        }

        if let PromptState::PickEditor {
            editors, selected, ..
        } = &mut self.prompt
        {
            match self.bindings.lookup(&key, BindingScope::Palette) {
                Some(Action::CloseOverlay) => self.prompt = PromptState::None,
                Some(Action::MoveDown) => {
                    if *selected + 1 < editors.len() {
                        *selected += 1;
                    }
                }
                Some(Action::MoveUp) => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                Some(Action::Confirm) => {
                    self.open_selected_pick_editor();
                }
                _ => {}
            }
            return Ok(false);
        }

        if let PromptState::ConfirmKillRunning(confirm_prompt) = &mut self.prompt {
            match self.bindings.lookup(&key, BindingScope::Dialog) {
                Some(Action::CloseOverlay) => {
                    let previous = confirm_prompt.previous.clone();
                    self.prompt = PromptState::KillRunning(previous);
                    self.set_info(
                        "Kill cancelled. Your running agents and companion terminals are unchanged.",
                    );
                }
                Some(Action::ToggleSelection) => {
                    confirm_prompt.confirm_selected = !confirm_prompt.confirm_selected;
                }
                Some(Action::Confirm) => {
                    let confirm = confirm_prompt.confirm_selected;
                    return Ok(self.resolve_confirm_kill_running(confirm));
                }
                _ => {}
            }
            return Ok(false);
        }

        if let PromptState::ConfirmDeleteAgent {
            confirm_selected, ..
        } = &mut self.prompt
        {
            match self.bindings.lookup(&key, BindingScope::Dialog) {
                Some(Action::CloseOverlay) => self.prompt = PromptState::None,
                Some(Action::ToggleSelection) => {
                    *confirm_selected = !*confirm_selected;
                }
                Some(Action::Confirm) => {
                    let confirm = *confirm_selected;
                    return Ok(self.resolve_confirm_delete_agent(confirm));
                }
                _ => {}
            }
        }

        if let PromptState::ConfirmQuit {
            confirm_selected, ..
        } = &mut self.prompt
        {
            match self.bindings.lookup(&key, BindingScope::Dialog) {
                Some(Action::CloseOverlay) => self.prompt = PromptState::None,
                Some(Action::ToggleSelection) => {
                    *confirm_selected = !*confirm_selected;
                }
                Some(Action::Confirm) => {
                    let confirm = *confirm_selected;
                    return Ok(self.resolve_confirm_quit(confirm));
                }
                _ => {}
            }
        }

        if let PromptState::ConfirmDiscardFile {
            confirm_selected, ..
        } = &mut self.prompt
        {
            match self.bindings.lookup(&key, BindingScope::Dialog) {
                Some(Action::CloseOverlay) => self.prompt = PromptState::None,
                Some(Action::ToggleSelection) => {
                    *confirm_selected = !*confirm_selected;
                }
                Some(Action::Confirm) => {
                    let confirm = *confirm_selected;
                    return Ok(self.resolve_confirm_discard_file(confirm));
                }
                _ => {}
            }
            return Ok(false);
        }

        if matches!(self.prompt, PromptState::EditMacros { .. }) {
            self.handle_edit_macros_key(key)?;
            return Ok(false);
        }

        if matches!(self.prompt, PromptState::NameNewAgent { .. }) {
            let is_plain_char = matches!(key.code, KeyCode::Char(_))
                && !key.modifiers.contains(KeyModifiers::CONTROL);
            let action = if is_plain_char {
                None
            } else {
                self.bindings.lookup(&key, BindingScope::Dialog)
            };

            match action {
                Some(Action::CloseOverlay) => {
                    self.prompt = PromptState::None;
                }
                Some(Action::Confirm) => {
                    // Extract the name from the input before taking ownership.
                    let name = if let PromptState::NameNewAgent { input, .. } = &self.prompt {
                        input.text.trim().to_string()
                    } else {
                        unreachable!()
                    };
                    if name.is_empty() {
                        self.set_error("Agent name cannot be empty.");
                        self.prompt = PromptState::None;
                        return Ok(false);
                    }
                    if !git::is_valid_agent_name(&name) {
                        self.set_error(
                            "Agent name may only contain letters, digits, dashes, underscores, \
                             or slashes. It cannot start with \"-\" or \"/\", end with \"/\", \
                             or contain \"//\".",
                        );
                        return Ok(false);
                    }
                    // Take ownership of the prompt to extract the request.
                    let old_prompt = std::mem::replace(&mut self.prompt, PromptState::None);
                    let PromptState::NameNewAgent { mut request, .. } = old_prompt else {
                        unreachable!()
                    };
                    let msg = match &request {
                        CreateAgentRequest::NewProject { project, .. } => {
                            format!(
                                "Creating a new agent worktree \"{name}\" for project \"{}\" and launching a fresh session...",
                                project.name
                            )
                        }
                        CreateAgentRequest::ForkSession { source_label, .. } => {
                            format!(
                                "Forking agent \"{source_label}\" as \"{name}\" by cloning its current worktree contents into a fresh session...",
                            )
                        }
                    };
                    match &mut request {
                        CreateAgentRequest::NewProject { custom_name, .. }
                        | CreateAgentRequest::ForkSession { custom_name, .. } => {
                            *custom_name = Some(name);
                        }
                    }
                    self.dispatch_create_agent_request(request, msg)?;
                }
                _ => {
                    if let PromptState::NameNewAgent { input, .. } = &mut self.prompt {
                        input.handle_key(key);
                    }
                }
            }
            return Ok(false);
        }

        if let PromptState::RenameSession {
            session_id,
            input,
            rename_branch,
        } = &mut self.prompt
        {
            let is_plain_char = matches!(key.code, KeyCode::Char(_))
                && !key.modifiers.contains(KeyModifiers::CONTROL);
            let action = if is_plain_char {
                None
            } else {
                self.bindings.lookup(&key, BindingScope::Dialog)
            };

            match action {
                Some(Action::CloseOverlay) => {
                    self.prompt = PromptState::None;
                }
                Some(Action::Confirm) => {
                    let id = session_id.clone();
                    let new_name = input.text.clone();
                    let also_rename_branch = *rename_branch;
                    self.prompt = PromptState::None;
                    self.apply_rename_session(&id, new_name, also_rename_branch);
                }
                Some(Action::ToggleSelection) => {
                    *rename_branch = !*rename_branch;
                }
                _ => {
                    input.handle_key(key);
                }
            }
            return Ok(false);
        }

        Ok(false)
    }

    fn handle_edit_macros_key(&mut self, key: KeyEvent) -> Result<bool> {
        let PromptState::EditMacros {
            entries,
            selected,
            editing,
        } = &mut self.prompt
        else {
            return Ok(false);
        };

        if let Some(edit_state) = editing {
            match edit_state.stage {
                MacroEditStage::EditName => {
                    if key.code == KeyCode::Esc {
                        *editing = None;
                        return Ok(false);
                    }
                    if key.code == KeyCode::Tab {
                        edit_state.surface = edit_state.surface.next();
                        return Ok(false);
                    }
                    if key.code == KeyCode::BackTab {
                        edit_state.surface = edit_state.surface.prev();
                        return Ok(false);
                    }
                    if key.code == KeyCode::Enter && !edit_state.name_input.is_empty() {
                        let name = edit_state.name_input.text.clone();
                        // For new macros, check for duplicate names
                        if edit_state.id.is_none() && entries.iter().any(|(n, _, _)| *n == name) {
                            self.set_warning(format!(
                                "Name \"{name}\" is already in use. Choose another."
                            ));
                            return Ok(false);
                        }
                        edit_state.stage = MacroEditStage::EditText;
                        return Ok(false);
                    }
                    edit_state.name_input.handle_key(key);
                }
                MacroEditStage::EditText => {
                    if key.code == KeyCode::Esc {
                        // Save the macro
                        let name = edit_state.name_input.text.clone();
                        let text = edit_state.text_input.text.clone();
                        let surface = edit_state.surface;
                        let old_id = edit_state.id.clone();

                        if text.is_empty() {
                            // Empty text — don't save
                            *editing = None;
                            return Ok(false);
                        }

                        // If renaming, remove the old entry
                        if let Some(ref old_name) = old_id
                            && *old_name != name
                        {
                            self.config.macros.entries.remove(old_name);
                        }

                        // Update config
                        self.config.macros.entries.insert(
                            name.clone(),
                            crate::config::MacroEntry {
                                text: text.clone(),
                                surface,
                            },
                        );

                        // Update the entries snapshot in PromptState
                        let PromptState::EditMacros {
                            entries, editing, ..
                        } = &mut self.prompt
                        else {
                            return Ok(false);
                        };
                        *editing = None;

                        // Update entries list
                        if let Some(old_name) = old_id {
                            if let Some(existing) =
                                entries.iter_mut().find(|(n, _, _)| *n == old_name)
                            {
                                existing.0 = name.clone();
                                existing.1 = text;
                                existing.2 = surface;
                            } else {
                                entries.push((name.clone(), text, surface));
                            }
                        } else {
                            entries.push((name.clone(), text, surface));
                        }
                        entries.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));

                        // Persist
                        let _ = crate::config::save_config(
                            &self.paths.config_path,
                            &self.config,
                            &self.bindings,
                        );
                        self.set_info(format!("Macro \"{name}\" saved."));
                        return Ok(false);
                    }
                    // In multiline mode, TextInput handles Enter/Up/Down
                    edit_state.text_input.handle_key(key);
                }
            }
            return Ok(false);
        }

        // List view — no active editing
        match key.code {
            KeyCode::Esc => {
                self.prompt = PromptState::None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !entries.is_empty() && *selected + 1 < entries.len() {
                    *selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if *selected > 0 {
                    *selected -= 1;
                }
            }
            KeyCode::Enter => {
                // Edit selected macro
                if let Some((name, text, surface)) = entries.get(*selected) {
                    let name = name.clone();
                    let text = text.clone();
                    let surface = *surface;
                    *editing = Some(MacroEditState {
                        id: Some(name.clone()),
                        name_input: TextInput::with_text(name),
                        text_input: TextInput::with_text(text).with_multiline(8),
                        surface,
                        stage: MacroEditStage::EditName,
                    });
                }
            }
            KeyCode::Char('n') => {
                // New macro
                *editing = Some(MacroEditState {
                    id: None,
                    name_input: TextInput::new(),
                    text_input: TextInput::new().with_multiline(8),
                    surface: MacroSurface::default(),
                    stage: MacroEditStage::EditName,
                });
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                // Delete selected macro
                if let Some((name, _, _)) = entries.get(*selected) {
                    let name = name.clone();
                    self.config.macros.entries.remove(&name);

                    let PromptState::EditMacros {
                        entries, selected, ..
                    } = &mut self.prompt
                    else {
                        return Ok(false);
                    };
                    entries.retain(|(n, _, _)| *n != name);
                    if *selected > 0 && *selected >= entries.len() {
                        *selected = entries.len().saturating_sub(1);
                    }

                    let _ = crate::config::save_config(
                        &self.paths.config_path,
                        &self.config,
                        &self.bindings,
                    );
                    self.set_info(format!("Macro \"{name}\" deleted."));
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_resize_key(&mut self, key: KeyEvent) {
        if let Some(action) = self.bindings.lookup(&key, BindingScope::Resize) {
            if self.focus == FocusPane::Files {
                match action {
                    Action::ResizeShrink => self.set_right_width_pct(self.right_width_pct + 2),
                    Action::ResizeGrow => {
                        self.set_right_width_pct(self.right_width_pct.saturating_sub(2))
                    }
                    _ => {}
                }
            } else {
                match action {
                    Action::ResizeShrink => {
                        self.set_left_width_pct(self.left_width_pct.saturating_sub(2))
                    }
                    Action::ResizeGrow => self.set_left_width_pct(self.left_width_pct + 2),
                    _ => {}
                }
            }
        }
    }

    fn set_left_width_pct(&mut self, left_width_pct: u16) {
        self.left_width_pct = clamp_left_width_pct(left_width_pct, self.right_width_pct);
        self.right_width_pct = clamp_right_width_pct(self.right_width_pct, self.left_width_pct);
    }

    fn set_right_width_pct(&mut self, right_width_pct: u16) {
        self.right_width_pct = clamp_right_width_pct(right_width_pct, self.left_width_pct);
        self.left_width_pct = clamp_left_width_pct(self.left_width_pct, self.right_width_pct);
    }

    fn overlay_row_at(
        rect: Rect,
        offset: usize,
        items: usize,
        column: u16,
        row: u16,
    ) -> Option<usize> {
        if !contains_point(rect, column, row) {
            return None;
        }
        let relative_row = usize::from(row.saturating_sub(rect.y));
        let index = offset.saturating_add(relative_row);
        (index < items).then_some(index)
    }

    fn prompt_mouse_target(&self, column: u16, row: u16) -> Option<PromptMouseTarget> {
        match self.overlay_layout.active {
            OverlayMouseLayout::None | OverlayMouseLayout::Help => None,
            OverlayMouseLayout::Command {
                input,
                list,
                items,
                offset,
                ..
            } => {
                if contains_point(input, column, row) {
                    Some(PromptMouseTarget::CommandInput)
                } else {
                    Self::overlay_row_at(list, offset, items, column, row)
                        .map(PromptMouseTarget::CommandItem)
                }
            }
            OverlayMouseLayout::BrowseProjects {
                input,
                list,
                items,
                offset,
                ..
            } => {
                if input.is_some_and(|rect| contains_point(rect, column, row)) {
                    Some(PromptMouseTarget::BrowseProjectInput)
                } else {
                    Self::overlay_row_at(list, offset, items, column, row)
                        .map(PromptMouseTarget::BrowseProjectItem)
                }
            }
            OverlayMouseLayout::PickEditor {
                list,
                items,
                offset,
                ..
            } => Self::overlay_row_at(list, offset, items, column, row)
                .map(PromptMouseTarget::PickEditorItem),
            OverlayMouseLayout::KillRunning {
                input,
                list,
                items,
                offset,
                cancel_button,
                hovered_button,
                selected_button,
                visible_button,
            } => {
                if input.is_some_and(|rect| contains_point(rect, column, row)) {
                    Some(PromptMouseTarget::RuntimeKillInput)
                } else if let Some(index) = Self::overlay_row_at(list, offset, items, column, row) {
                    Some(PromptMouseTarget::RuntimeKillItem(index))
                } else if contains_point(cancel_button, column, row) {
                    Some(PromptMouseTarget::RuntimeKillCancel)
                } else if contains_point(hovered_button, column, row) {
                    Some(PromptMouseTarget::RuntimeKillHovered)
                } else if contains_point(selected_button, column, row) {
                    Some(PromptMouseTarget::RuntimeKillSelected)
                } else if contains_point(visible_button, column, row) {
                    Some(PromptMouseTarget::RuntimeKillVisible)
                } else {
                    None
                }
            }
            OverlayMouseLayout::ConfirmKillRunning {
                cancel_button,
                kill_button,
            } => {
                if contains_point(cancel_button, column, row) {
                    Some(PromptMouseTarget::ConfirmKillCancel)
                } else if contains_point(kill_button, column, row) {
                    Some(PromptMouseTarget::ConfirmKillConfirm)
                } else {
                    None
                }
            }
            OverlayMouseLayout::ConfirmDeleteAgent {
                cancel_button,
                delete_button,
            } => {
                if contains_point(cancel_button, column, row) {
                    Some(PromptMouseTarget::ConfirmDeleteCancel)
                } else if contains_point(delete_button, column, row) {
                    Some(PromptMouseTarget::ConfirmDeleteConfirm)
                } else {
                    None
                }
            }
            OverlayMouseLayout::ConfirmQuit {
                cancel_button,
                quit_button,
            } => {
                if contains_point(cancel_button, column, row) {
                    Some(PromptMouseTarget::ConfirmQuitCancel)
                } else if contains_point(quit_button, column, row) {
                    Some(PromptMouseTarget::ConfirmQuitConfirm)
                } else {
                    None
                }
            }
            OverlayMouseLayout::ConfirmDiscardFile {
                cancel_button,
                discard_button,
            } => {
                if contains_point(cancel_button, column, row) {
                    Some(PromptMouseTarget::ConfirmDiscardCancel)
                } else if contains_point(discard_button, column, row) {
                    Some(PromptMouseTarget::ConfirmDiscardConfirm)
                } else {
                    None
                }
            }
            OverlayMouseLayout::RenameSession { input } => {
                contains_point(input, column, row).then_some(PromptMouseTarget::RenameInput)
            }
            OverlayMouseLayout::NameNewAgent { input } => {
                contains_point(input, column, row).then_some(PromptMouseTarget::NameNewAgentInput)
            }
        }
    }

    fn mouse_target(&self, column: u16, row: u16) -> Option<MouseTarget> {
        if !matches!(self.fullscreen_overlay, FullscreenOverlay::None) {
            return self
                .mouse_layout
                .agent_term
                .filter(|rect| contains_point(*rect, column, row))
                .map(|_| MouseTarget::Center);
        }

        if contains_point(self.mouse_layout.left_list, column, row) {
            if self.left_items().is_empty() {
                return Some(MouseTarget::LeftPane);
            }
            let index = usize::from(row.saturating_sub(self.mouse_layout.left_list.y));
            if index < self.left_items().len() {
                return Some(MouseTarget::LeftRow(index));
            }
            return Some(MouseTarget::LeftPane);
        }

        {
            let tl = self.mouse_layout.terminal_list;
            if tl.width > 0 && tl.height > 0 && contains_point(tl, column, row) {
                let index = usize::from(row.saturating_sub(tl.y));
                let term_count = self.terminal_items().len();
                if index < term_count {
                    return Some(MouseTarget::TerminalRow(index));
                }
                return Some(MouseTarget::TerminalPane);
            }
        }

        if contains_point(self.mouse_layout.left, column, row) {
            return Some(MouseTarget::LeftPane);
        }

        if let Some(area) = self.mouse_layout.unstaged_list
            && contains_point(area, column, row)
        {
            let index = usize::from(row.saturating_sub(area.y));
            let file_index = (index < self.unstaged_files.len()).then_some(index);
            return Some(MouseTarget::UnstagedFile(file_index));
        }

        if let Some(area) = self.mouse_layout.staged_list
            && contains_point(area, column, row)
        {
            let index = usize::from(row.saturating_sub(area.y));
            let file_index = (index < self.staged_files.len()).then_some(index);
            return Some(MouseTarget::StagedFile(file_index));
        }

        if let Some(area) = self.mouse_layout.commit_area
            && contains_point(area, column, row)
        {
            if self
                .mouse_layout
                .commit_text_area
                .is_some_and(|text_area| contains_point(text_area, column, row))
            {
                return Some(MouseTarget::CommitText);
            }
            return Some(MouseTarget::CommitChrome);
        }

        if contains_point(self.mouse_layout.right, column, row) {
            return Some(MouseTarget::FilesPane);
        }

        if contains_point(self.mouse_layout.center, column, row) {
            return Some(MouseTarget::Center);
        }

        None
    }

    fn resize_drag_at_mouse(&self, column: u16, row: u16) -> Option<ResizeDragState> {
        let body = self.mouse_layout.body;
        if !contains_point(body, column, row) {
            return None;
        }

        let left_edge = self.mouse_layout.left.x + self.mouse_layout.left.width.saturating_sub(1);
        let center_left = self.mouse_layout.center.x;
        let center_right =
            self.mouse_layout.center.x + self.mouse_layout.center.width.saturating_sub(1);
        let right_left = self.mouse_layout.right.x;

        if !self.left_collapsed && (column == left_edge || column == center_left) {
            return Some(ResizeDragState::LeftDivider);
        }

        if !self.right_hidden && (column == center_right || column == right_left) {
            return Some(ResizeDragState::RightDivider);
        }

        // Horizontal divider between Projects and Terminals sections.
        let tl = self.mouse_layout.terminal_list;
        if tl.width > 0 && tl.height > 0 {
            let left = self.mouse_layout.left;
            let divider_row = tl.y.saturating_sub(1);
            if row == divider_row && column >= left.x && column < left.x + left.width {
                return Some(ResizeDragState::TerminalDivider);
            }
        }

        // Horizontal dividers inside the right pane.
        //
        // The inner content rects (unstaged_list, staged_list) exclude the
        // surrounding block borders, so the gap between two adjacent rects is
        // typically only 1-2 rows of border chrome.  We extend the hit zone
        // outward from each content rect by 1 row to cover the border that
        // belongs to each block, making the target at least 3 rows wide
        // (bottom border + gap + top border) without overlapping the content.

        // Between Unstaged and Staged.
        if let (Some(unstaged), Some(staged)) = (
            self.mouse_layout.unstaged_list,
            self.mouse_layout.staged_list,
        ) {
            let right = self.mouse_layout.right;
            // The content rects don't include their enclosing block borders.
            // Extend one row past each content edge to cover the border row.
            let hit_top = unstaged.y + unstaged.height; // first row after unstaged content (border)
            let hit_bottom = staged.y.saturating_sub(1); // last row before staged content (border)
            if row >= hit_top
                && row <= hit_bottom
                && column >= right.x
                && column < right.x + right.width
            {
                return Some(ResizeDragState::StagedDivider);
            }
        }

        // Between Staged Changes and Commit Message.
        // staged_list is an inner rect; commit_area is an outer rect (includes border).
        if let (Some(staged), Some(commit)) =
            (self.mouse_layout.staged_list, self.mouse_layout.commit_area)
        {
            let right = self.mouse_layout.right;
            let hit_top = staged.y + staged.height; // first row after staged content (border)
            let hit_bottom = commit.y; // top border row of commit block
            if row >= hit_top
                && row <= hit_bottom
                && column >= right.x
                && column < right.x + right.width
            {
                return Some(ResizeDragState::CommitDivider);
            }
        }

        None
    }

    fn set_left_selection(&mut self, index: usize) {
        if index >= self.left_items().len() {
            return;
        }
        self.focus = FocusPane::Left;
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        if self.selected_left != index {
            self.selected_left = index;
            self.reload_changed_files();
        }
    }

    fn register_mouse_click(
        &mut self,
        target: MouseClickTarget,
        item_index: Option<usize>,
    ) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_mouse_click
            && last.target == target
            && last.item_index == item_index
            && now.duration_since(last.at) <= DOUBLE_CLICK_THRESHOLD
        {
            self.last_mouse_click = None;
            return true;
        }

        self.last_mouse_click = Some(RecentMouseClick {
            target,
            item_index,
            at: now,
        });
        false
    }

    fn set_command_palette_selection(&mut self, index: usize) {
        let count = match &self.prompt {
            PromptState::Command { input, .. } => self.bindings.filtered_palette(&input.text).len(),
            _ => 0,
        };
        if count == 0 {
            return;
        }
        if let PromptState::Command { selected, .. } = &mut self.prompt {
            *selected = index.min(count.saturating_sub(1));
        }
    }

    fn set_command_palette_cursor_from_mouse(&mut self, column: u16) {
        let input_area = match self.overlay_layout.active {
            OverlayMouseLayout::Command { input, .. } => input,
            _ => return,
        };
        if let PromptState::Command { input, .. } = &mut self.prompt {
            let prefix_width = 2; // "> "
            input.cursor =
                cursor_from_single_line_position(&input.text, input_area, prefix_width, column);
        }
    }

    fn execute_selected_command_palette(&mut self) {
        let command = if let PromptState::Command {
            input, selected, ..
        } = &self.prompt
        {
            if let Some(binding) = self.bindings.filtered_palette(&input.text).get(*selected) {
                binding.palette_name.unwrap().to_string()
            } else {
                input.text.trim().to_string()
            }
        } else {
            String::new()
        };
        self.prompt = PromptState::None;
        if let Err(e) = self.execute_command(command) {
            self.set_error(format!("{e:#}"));
        }
    }

    fn visible_browser_entries(&self) -> Vec<BrowserEntry> {
        if let PromptState::BrowseProjects {
            entries, filter, ..
        } = &self.prompt
        {
            if filter.is_empty() {
                entries.clone()
            } else {
                let needle = filter.text.to_lowercase();
                entries
                    .iter()
                    .filter(|entry| entry.label.to_lowercase().contains(&needle))
                    .cloned()
                    .collect()
            }
        } else {
            Vec::new()
        }
    }

    fn set_browser_selection(&mut self, index: usize) {
        let count = self.visible_browser_entries().len();
        if count == 0 {
            return;
        }
        if let PromptState::BrowseProjects { selected, .. } = &mut self.prompt {
            *selected = index.min(count.saturating_sub(1));
        }
    }

    fn set_browser_input_cursor_from_mouse(&mut self, column: u16) {
        let input_area = match self.overlay_layout.active {
            OverlayMouseLayout::BrowseProjects {
                input: Some(input), ..
            } => input,
            _ => return,
        };
        if let PromptState::BrowseProjects {
            filter,
            searching,
            editing_path,
            path_input,
            ..
        } = &mut self.prompt
        {
            if *editing_path {
                path_input.cursor =
                    cursor_from_single_line_position(&path_input.text, input_area, 4, column);
            } else {
                filter.cursor =
                    cursor_from_single_line_position(&filter.text, input_area, 2, column);
                *searching = true;
            }
        }
    }

    fn set_kill_running_hovered(&mut self, visible_index: usize) {
        if let PromptState::KillRunning(prompt) = &mut self.prompt {
            let count = Self::visible_kill_running_indices(prompt).len();
            if count == 0 {
                prompt.hovered_visible_index = 0;
                return;
            }
            prompt.hovered_visible_index = visible_index.min(count.saturating_sub(1));
            prompt.focus = KillRunningFocus::List;
        }
    }

    fn set_kill_running_search_cursor_from_mouse(&mut self, column: u16) {
        let input_area = match self.overlay_layout.active {
            OverlayMouseLayout::KillRunning {
                input: Some(input), ..
            } => input,
            _ => return,
        };
        if let PromptState::KillRunning(prompt) = &mut self.prompt {
            prompt.filter.cursor =
                cursor_from_single_line_position(&prompt.filter.text, input_area, 2, column);
            prompt.searching = true;
            prompt.focus = KillRunningFocus::List;
        }
    }

    fn toggle_hovered_kill_running_selection(&mut self) {
        let target_id = match &self.prompt {
            PromptState::KillRunning(prompt) => {
                let visible = Self::visible_kill_running_indices(prompt);
                visible
                    .get(prompt.hovered_visible_index)
                    .and_then(|&index| prompt.runtimes.get(index))
                    .map(|runtime| runtime.id.clone())
            }
            _ => None,
        };

        let Some(target_id) = target_id else {
            self.set_error(
                "No running agent or terminal is highlighted. Move to a visible row first.",
            );
            return;
        };

        if let PromptState::KillRunning(prompt) = &mut self.prompt
            && !prompt.selected_ids.insert(target_id.clone())
        {
            prompt.selected_ids.remove(&target_id);
        }
    }

    fn execute_kill_running_footer_action(
        &mut self,
        action: KillRunningFooterAction,
    ) -> Result<()> {
        let enabled = match &self.prompt {
            PromptState::KillRunning(prompt) => Self::kill_running_footer_enabled(prompt, action),
            _ => true,
        };
        if !enabled {
            if matches!(action, KillRunningFooterAction::Selected) {
                self.set_info(
                    "Select one or more runtimes before using Kill Selected. Press Space to mark the highlighted row.",
                );
            }
            return Ok(());
        }
        match action {
            KillRunningFooterAction::Cancel => {
                self.prompt = PromptState::None;
                self.set_info("Closed Kill Running. No agents or terminals were killed.");
            }
            _ => {
                if let Some(action) = action.action() {
                    self.open_confirm_kill_running_action(action)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn kill_running_footer_enabled(
        prompt: &KillRunningPrompt,
        action: KillRunningFooterAction,
    ) -> bool {
        match action {
            KillRunningFooterAction::Cancel => true,
            KillRunningFooterAction::Selected => !prompt.selected_ids.is_empty(),
            KillRunningFooterAction::Hovered | KillRunningFooterAction::Visible => {
                !Self::visible_kill_running_indices(prompt).is_empty()
            }
        }
    }

    fn next_kill_running_footer_action(
        prompt: &KillRunningPrompt,
        current: Option<KillRunningFooterAction>,
        forward: bool,
    ) -> KillRunningFocus {
        const ACTIONS: [KillRunningFooterAction; 4] = [
            KillRunningFooterAction::Cancel,
            KillRunningFooterAction::Hovered,
            KillRunningFooterAction::Selected,
            KillRunningFooterAction::Visible,
        ];

        let enabled: Vec<KillRunningFooterAction> = ACTIONS
            .into_iter()
            .filter(|action| Self::kill_running_footer_enabled(prompt, *action))
            .collect();

        if enabled.is_empty() {
            return KillRunningFocus::List;
        }

        match current {
            None => {
                if forward {
                    KillRunningFocus::Footer(enabled[0])
                } else {
                    KillRunningFocus::Footer(*enabled.last().unwrap())
                }
            }
            Some(current) => {
                let Some(index) = enabled.iter().position(|action| *action == current) else {
                    return if forward {
                        KillRunningFocus::Footer(enabled[0])
                    } else {
                        KillRunningFocus::Footer(*enabled.last().unwrap())
                    };
                };
                if forward {
                    if index + 1 < enabled.len() {
                        KillRunningFocus::Footer(enabled[index + 1])
                    } else {
                        KillRunningFocus::List
                    }
                } else if index > 0 {
                    KillRunningFocus::Footer(enabled[index - 1])
                } else {
                    KillRunningFocus::List
                }
            }
        }
    }

    fn open_selected_browser_entry(&mut self) {
        let visible = self.visible_browser_entries();
        let mut browse_to: Option<PathBuf> = None;
        if let PromptState::BrowseProjects {
            current_dir,
            entries,
            loading,
            selected,
            filter,
            ..
        } = &mut self.prompt
            && let Some(entry) = visible.get(*selected)
        {
            let new_dir = entry.path.clone();
            *current_dir = new_dir.clone();
            entries.clear();
            *loading = true;
            *selected = 0;
            filter.clear();
            browse_to = Some(new_dir);
        }
        if let Some(dir) = browse_to {
            self.spawn_browser_entries(&dir);
        }
    }

    fn set_pick_editor_selection(&mut self, index: usize) {
        let count = match &self.prompt {
            PromptState::PickEditor { editors, .. } => editors.len(),
            _ => 0,
        };
        if count == 0 {
            return;
        }
        if let PromptState::PickEditor { selected, .. } = &mut self.prompt {
            *selected = index.min(count.saturating_sub(1));
        }
    }

    fn open_selected_pick_editor(&mut self) {
        let (editor, worktree, label) = if let PromptState::PickEditor {
            session_label,
            worktree_path,
            editors,
            selected,
        } = &self.prompt
        {
            (
                editors.get(*selected).cloned(),
                worktree_path.clone(),
                session_label.clone(),
            )
        } else {
            return;
        };
        self.prompt = PromptState::None;
        if let Some(editor) = editor
            && let Err(e) = self.open_worktree_in_editor(&worktree, &label, &editor)
        {
            self.set_error(format!("{e:#}"));
        }
    }

    fn resolve_confirm_delete_agent(&mut self, confirm: bool) -> bool {
        let session_id = match &self.prompt {
            PromptState::ConfirmDeleteAgent { session_id, .. } => session_id.clone(),
            _ => return false,
        };
        self.prompt = PromptState::None;
        if confirm && let Err(e) = self.do_delete_session(&session_id) {
            self.set_error(format!("{e:#}"));
        }
        false
    }

    fn resolve_confirm_kill_running(&mut self, confirm: bool) -> bool {
        let confirm_prompt = match &self.prompt {
            PromptState::ConfirmKillRunning(confirm_prompt) => confirm_prompt.clone(),
            _ => return false,
        };
        if !confirm {
            self.prompt = PromptState::KillRunning(confirm_prompt.previous);
            self.set_info(
                "Kill cancelled. Your running agents and companion terminals are unchanged.",
            );
            return false;
        }

        self.prompt = PromptState::None;
        let requested = confirm_prompt.target_ids.len();
        let (agents, terminals) = self.kill_runtime_targets(&confirm_prompt.target_ids);
        let killed = agents + terminals;
        let already_gone = requested.saturating_sub(killed);
        if killed == 0 {
            self.set_warning(
                "The selected agents or terminals were already gone, so there was nothing left to kill. Refresh Kill Running if you want to review the current runtime list.",
            );
            return false;
        }

        let mut pieces = Vec::new();
        if agents > 0 {
            pieces.push(format!(
                "{agents} agent{}",
                if agents == 1 { "" } else { "s" }
            ));
        }
        if terminals > 0 {
            pieces.push(format!(
                "{terminals} terminal{}",
                if terminals == 1 { "" } else { "s" }
            ));
        }
        if already_gone > 0 {
            self.set_warning(format!(
                "Killed {}. {} selected runtime{} were already gone. In-progress CLI work was stopped, but the worktree files are still available for review or relaunch.",
                pieces.join(" and "),
                already_gone,
                if already_gone == 1 { "" } else { "s" }
            ));
        } else {
            self.set_info(format!(
                "Killed {}. In-progress CLI work was stopped, but the worktree files are still available for review or relaunch.",
                pieces.join(" and ")
            ));
        }
        false
    }

    fn resolve_confirm_quit(&mut self, confirm: bool) -> bool {
        if matches!(self.prompt, PromptState::ConfirmQuit { .. }) {
            self.prompt = PromptState::None;
        }
        confirm
    }

    fn resolve_confirm_discard_file(&mut self, confirm: bool) -> bool {
        let (file_path, is_untracked) = match &self.prompt {
            PromptState::ConfirmDiscardFile {
                file_path,
                is_untracked,
                ..
            } => (file_path.clone(), *is_untracked),
            _ => return false,
        };
        self.prompt = PromptState::None;
        if confirm && let Some(session) = self.selected_session() {
            let worktree = PathBuf::from(&session.worktree_path);
            match git::discard_file(&worktree, &file_path, is_untracked) {
                Ok(()) => {
                    self.set_info(format!(
                        "Discarded changes to \"{file_path}\". File restored to last committed state."
                    ));
                    self.reload_changed_files();
                }
                Err(e) => self.set_error(format!("Discard failed: {e}")),
            }
        }
        false
    }

    fn set_rename_cursor_from_mouse(&mut self, column: u16) {
        let input_area = match self.overlay_layout.active {
            OverlayMouseLayout::RenameSession { input } => input,
            _ => return,
        };
        if let PromptState::RenameSession { input, .. } = &mut self.prompt {
            input.cursor = cursor_from_single_line_position(&input.text, input_area, 0, column);
        }
    }

    fn set_name_new_agent_cursor_from_mouse(&mut self, column: u16) {
        let input_area = match self.overlay_layout.active {
            OverlayMouseLayout::NameNewAgent { input } => input,
            _ => return,
        };
        if let PromptState::NameNewAgent { input, .. } = &mut self.prompt {
            input.cursor = cursor_from_single_line_position(&input.text, input_area, 0, column);
        }
    }

    fn handle_prompt_mouse(&mut self, mouse: MouseEvent) -> bool {
        if let PromptState::ResourceMonitor {
            scroll_offset,
            rows,
            ..
        } = &mut self.prompt
        {
            let max_offset = rows.len().saturating_sub(1) as u16;
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    *scroll_offset = scroll_offset.saturating_sub(3);
                }
                MouseEventKind::ScrollDown => {
                    *scroll_offset = (*scroll_offset + 3).min(max_offset);
                }
                _ => {}
            }
            return false;
        }

        if let PromptState::DebugInput {
            lines,
            scroll_offset,
        } = &mut self.prompt
        {
            // Scroll wheel navigates history without logging.
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    *scroll_offset = scroll_offset.saturating_sub(3);
                    return false;
                }
                MouseEventKind::ScrollDown => {
                    *scroll_offset = (*scroll_offset + 3).min(lines.len() as u16);
                    return false;
                }
                _ => {}
            }

            let kind_label = format!("{:?}", mouse.kind);
            let ts = Local::now().format("%H:%M:%S%.3f").to_string();
            lines.push(Line::from(vec![
                Span::styled(ts, Style::default().add_modifier(Modifier::DIM)),
                Span::raw(" │ "),
                Span::styled(
                    "Mouse",
                    Style::default().fg(self.theme.help_section_header_fg),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("{:<18}", kind_label),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" │ "),
                Span::styled(
                    format!("col={} row={}", mouse.column, mouse.row),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ]));

            // Auto-scroll to bottom.
            let total = lines.len() as u16;
            *scroll_offset = total;

            return false;
        }

        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }

        let Some(target) = self.prompt_mouse_target(mouse.column, mouse.row) else {
            return false;
        };

        match target {
            PromptMouseTarget::CommandInput => {
                self.set_command_palette_cursor_from_mouse(mouse.column);
            }
            PromptMouseTarget::CommandItem(index) => {
                let double_click =
                    self.register_mouse_click(MouseClickTarget::CommandPalette, Some(index));
                self.set_command_palette_selection(index);
                if double_click {
                    self.execute_selected_command_palette();
                }
            }
            PromptMouseTarget::BrowseProjectInput => {
                self.set_browser_input_cursor_from_mouse(mouse.column);
            }
            PromptMouseTarget::BrowseProjectItem(index) => {
                let double_click =
                    self.register_mouse_click(MouseClickTarget::CommandPalette, Some(index));
                self.set_browser_selection(index);
                if double_click {
                    self.open_selected_browser_entry();
                }
            }
            PromptMouseTarget::PickEditorItem(index) => {
                let double_click =
                    self.register_mouse_click(MouseClickTarget::CommandPalette, Some(index));
                self.set_pick_editor_selection(index);
                if double_click {
                    self.open_selected_pick_editor();
                }
            }
            PromptMouseTarget::RuntimeKillInput => {
                self.set_kill_running_search_cursor_from_mouse(mouse.column);
            }
            PromptMouseTarget::RuntimeKillItem(index) => {
                self.set_kill_running_hovered(index);
                self.toggle_hovered_kill_running_selection();
            }
            PromptMouseTarget::RuntimeKillCancel => {
                if let Err(e) =
                    self.execute_kill_running_footer_action(KillRunningFooterAction::Cancel)
                {
                    self.set_error(format!("{e:#}"));
                }
            }
            PromptMouseTarget::RuntimeKillHovered => {
                if let Err(e) =
                    self.execute_kill_running_footer_action(KillRunningFooterAction::Hovered)
                {
                    self.set_error(format!("{e:#}"));
                }
            }
            PromptMouseTarget::RuntimeKillSelected => {
                if let Err(e) =
                    self.execute_kill_running_footer_action(KillRunningFooterAction::Selected)
                {
                    self.set_error(format!("{e:#}"));
                }
            }
            PromptMouseTarget::RuntimeKillVisible => {
                if let Err(e) =
                    self.execute_kill_running_footer_action(KillRunningFooterAction::Visible)
                {
                    self.set_error(format!("{e:#}"));
                }
            }
            PromptMouseTarget::ConfirmKillCancel => {
                return self.resolve_confirm_kill_running(false);
            }
            PromptMouseTarget::ConfirmKillConfirm => {
                return self.resolve_confirm_kill_running(true);
            }
            PromptMouseTarget::ConfirmDeleteCancel => {
                return self.resolve_confirm_delete_agent(false);
            }
            PromptMouseTarget::ConfirmDeleteConfirm => {
                return self.resolve_confirm_delete_agent(true);
            }
            PromptMouseTarget::ConfirmQuitCancel => return self.resolve_confirm_quit(false),
            PromptMouseTarget::ConfirmQuitConfirm => return self.resolve_confirm_quit(true),
            PromptMouseTarget::ConfirmDiscardCancel => {
                return self.resolve_confirm_discard_file(false);
            }
            PromptMouseTarget::ConfirmDiscardConfirm => {
                return self.resolve_confirm_discard_file(true);
            }
            PromptMouseTarget::RenameInput => {
                self.set_rename_cursor_from_mouse(mouse.column);
            }
            PromptMouseTarget::NameNewAgentInput => {
                self.set_name_new_agent_cursor_from_mouse(mouse.column);
            }
        }

        false
    }

    fn activate_selected_left_item(&mut self) -> Result<()> {
        match self.left_items().get(self.selected_left) {
            Some(LeftItem::Project(project_index)) => {
                let project_id = self.projects[*project_index].id.clone();
                let has_sessions = self.sessions.iter().any(|s| s.project_id == project_id);
                if has_sessions {
                    if self.collapsed_projects.contains(&project_id) {
                        self.collapsed_projects.remove(&project_id);
                        self.rebuild_left_items();
                    }
                    if let Some(pos) = self.left_items().iter().position(|item| {
                        matches!(item, LeftItem::Session(si) if self.sessions[*si].project_id == project_id)
                    }) {
                        self.selected_left = pos;
                        self.center_mode = CenterMode::Agent;
                        self.focus = FocusPane::Center;
                        self.reload_changed_files();
                        if self
                            .selected_session()
                            .map(|s| self.providers.contains_key(&s.id))
                            .unwrap_or(false)
                        {
                            self.input_target = InputTarget::Agent;
                            self.fullscreen_overlay = FullscreenOverlay::Agent;
                        } else if self.selected_session().is_some() {
                            self.reconnect_selected_session()?;
                        }
                    }
                } else {
                    self.create_agent_for_selected_project()?;
                }
            }
            Some(LeftItem::Session(_)) => {
                self.center_mode = CenterMode::Agent;
                self.focus = FocusPane::Center;
                self.reload_changed_files();
                if self
                    .selected_session()
                    .map(|s| self.providers.contains_key(&s.id))
                    .unwrap_or(false)
                {
                    self.input_target = InputTarget::Agent;
                    self.fullscreen_overlay = FullscreenOverlay::Agent;
                } else if self.selected_session().is_some() {
                    self.reconnect_selected_session()?;
                }
            }
            None => {}
        }
        Ok(())
    }

    fn activate_selected_left_item_from_mouse(&mut self) {
        if let Err(err) = self.activate_selected_left_item() {
            self.set_error(format!("Mouse activation failed: {err}"));
        }
    }

    pub(crate) fn activate_center_agent(&mut self) -> Result<()> {
        if !matches!(self.center_mode, CenterMode::Agent) {
            return Ok(());
        }
        if self.selected_session().is_some()
            && self
                .selected_session()
                .map(|s| self.providers.contains_key(&s.id))
                .unwrap_or(false)
        {
            self.reset_pty_scrollback();
            self.input_target = InputTarget::Agent;
            self.fullscreen_overlay = FullscreenOverlay::Agent;
            let exit_key = self.bindings.label_for(Action::ExitInteractive);
            self.set_info(format!(
                "Interactive mode. Keys forwarded to agent. {exit_key} exits."
            ));
        } else if self.selected_session().is_some() {
            self.reconnect_selected_session()?;
        } else {
            let n = self.bindings.label_for(Action::NewAgent);
            self.set_error(format!(
                "No agent selected. Press \"{n}\" to create a new one."
            ));
        }
        Ok(())
    }

    fn activate_center_agent_from_mouse(&mut self) {
        if let Err(err) = self.activate_center_agent() {
            self.set_error(format!("Mouse activation failed: {err}"));
        }
    }

    fn open_selected_file_diff_from_mouse(&mut self) {
        if let Err(err) = self.open_diff_for_selected_file() {
            self.set_error(format!("Mouse activation failed: {err}"));
        }
    }

    fn set_file_selection(&mut self, section: RightSection, index: Option<usize>) {
        self.focus = FocusPane::Files;
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.right_section = section;
        if let Some(index) = index {
            self.files_index = index;
        }
        self.clamp_files_cursor();
    }

    fn engage_commit_input(&mut self) {
        self.focus = FocusPane::Files;
        self.right_section = RightSection::CommitInput;
        self.input_target = InputTarget::CommitMessage;
        self.fullscreen_overlay = FullscreenOverlay::None;
    }

    fn set_commit_cursor_from_mouse(&mut self, column: u16, row: u16) {
        let Some(text_area) = self.mouse_layout.commit_text_area else {
            self.commit_input.move_end();
            return;
        };
        let display_row = usize::from(row.saturating_sub(text_area.y));
        let display_col = usize::from(column.saturating_sub(text_area.x));
        self.commit_input
            .set_cursor_from_display_pos(display_row, display_col);
    }

    fn scroll_commit_input(&mut self, down: bool) {
        let delta = if down {
            MOUSE_WHEEL_LINES as isize
        } else {
            -(MOUSE_WHEEL_LINES as isize)
        };
        self.commit_input.scroll_by(delta);
    }

    fn scroll_file_selection(&mut self, section: RightSection, down: bool) {
        self.set_file_selection(section, Some(self.files_index));
        if self.right_section == RightSection::CommitInput {
            return;
        }

        let len = self.current_files_len();
        if len == 0 {
            return;
        }

        if down {
            if self.files_index + 1 < len {
                self.files_index += 1;
            }
        } else if self.files_index > 0 {
            self.files_index -= 1;
        }
    }

    fn handle_left_mouse_wheel(&mut self, down: bool, column: u16, row: u16) {
        let target_index = match self.mouse_target(column, row) {
            Some(MouseTarget::LeftRow(index)) => index,
            _ => self.selected_left,
        };
        self.set_left_selection(target_index);

        if down {
            if self.selected_left + 1 < self.left_items().len() {
                self.selected_left += 1;
                self.reload_changed_files();
            }
        } else if self.selected_left > 0 {
            self.selected_left -= 1;
            self.reload_changed_files();
        }
    }

    fn handle_center_mouse_wheel(&mut self, mouse: MouseEvent) {
        self.focus = FocusPane::Center;
        if let CenterMode::Diff { ref mut scroll, .. } = self.center_mode {
            let delta = MOUSE_WHEEL_LINES as u16;
            if matches!(mouse.kind, MouseEventKind::ScrollDown) {
                let max_scroll = self
                    .last_diff_visual_lines
                    .saturating_sub(self.last_diff_height.max(1));
                *scroll = (*scroll + delta).min(max_scroll);
            } else if matches!(mouse.kind, MouseEventKind::ScrollUp) {
                *scroll = scroll.saturating_sub(delta);
            }
            return;
        }

        let Some(provider) = self.selected_terminal_surface_client() else {
            return;
        };

        // Always handle scroll as host scrollback — never forward to the child
        // process. The outer terminal's EnableMouseCapture means dux owns the
        // mouse; forwarding scroll events causes confusion.
        provider.scroll(
            matches!(mouse.kind, MouseEventKind::ScrollUp),
            MOUSE_WHEEL_LINES,
        );
    }

    fn update_dragged_panes(&mut self, column: u16, row: u16) {
        let body = self.mouse_layout.body;
        if body.width == 0 {
            return;
        }

        let body_right = body.x + body.width;
        match self.mouse_drag {
            Some(ResizeDragState::LeftDivider) => {
                let columns = column
                    .saturating_sub(body.x)
                    .saturating_add(1)
                    .clamp(1, body.width);
                self.set_left_width_pct(pct_from_columns(columns, body.width));
            }
            Some(ResizeDragState::RightDivider) => {
                let columns = body_right.saturating_sub(column).clamp(1, body.width);
                self.set_right_width_pct(pct_from_columns(columns, body.width));
            }
            Some(ResizeDragState::TerminalDivider) => {
                let left = self.mouse_layout.left;
                if left.height > 0 {
                    // Terminal height = distance from mouse row to bottom of left pane.
                    let left_bottom = left.y + left.height;
                    let term_rows = left_bottom.saturating_sub(row).clamp(1, left.height);
                    let pct = pct_from_columns(term_rows, left.height); // reuse same % helper
                    self.terminal_pane_height_pct =
                        pct.clamp(MIN_TERMINAL_PANE_HEIGHT_PCT, MAX_TERMINAL_PANE_HEIGHT_PCT);
                }
            }
            Some(ResizeDragState::StagedDivider) => {
                let right = self.mouse_layout.right;
                if right.height > 0 {
                    // Staged height = distance from mouse row to bottom of right pane.
                    let right_bottom = right.y + right.height;
                    let staged_rows = right_bottom.saturating_sub(row).clamp(1, right.height);
                    let pct = pct_from_columns(staged_rows, right.height);
                    self.staged_pane_height_pct =
                        pct.clamp(MIN_STAGED_PANE_HEIGHT_PCT, MAX_STAGED_PANE_HEIGHT_PCT);
                }
            }
            Some(ResizeDragState::CommitDivider) => {
                // The commit divider resizes the split between "Staged Changes"
                // and "Commit Message".  We compute relative to the staged
                // sub-area of the right pane.  The sub-area spans from where
                // the staged section starts to the bottom of the right pane.
                // Using the right pane bottom as reference keeps the
                // calculation stable even when layout rects are stale during
                // multi-frame drags.
                if let Some(staged) = self.mouse_layout.staged_list {
                    let right = self.mouse_layout.right;
                    // Sub-area = staged block top to right pane bottom.
                    let sub_top = staged.y.saturating_sub(1); // include staged border
                    let sub_bottom = right.y + right.height;
                    let sub_height = sub_bottom.saturating_sub(sub_top);
                    if sub_height > 0 {
                        let commit_rows = sub_bottom.saturating_sub(row).clamp(1, sub_height);
                        let pct = pct_from_columns(commit_rows, sub_height);
                        self.commit_pane_height_pct =
                            pct.clamp(MIN_COMMIT_PANE_HEIGHT_PCT, MAX_COMMIT_PANE_HEIGHT_PCT);
                    }
                }
            }
            None => {}
        }
    }

    fn persist_pane_widths(&mut self) {
        if self.config.ui.left_width_pct != self.left_width_pct
            || self.config.ui.right_width_pct != self.right_width_pct
            || self.config.ui.terminal_pane_height_pct != self.terminal_pane_height_pct
            || self.config.ui.staged_pane_height_pct != self.staged_pane_height_pct
            || self.config.ui.commit_pane_height_pct != self.commit_pane_height_pct
        {
            self.config.ui.left_width_pct = self.left_width_pct;
            self.config.ui.right_width_pct = self.right_width_pct;
            self.config.ui.terminal_pane_height_pct = self.terminal_pane_height_pct;
            self.config.ui.staged_pane_height_pct = self.staged_pane_height_pct;
            self.config.ui.commit_pane_height_pct = self.commit_pane_height_pct;
            let _ = save_config(&self.paths.config_path, &self.config, &self.bindings);
        }
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(self.prompt, PromptState::None) {
            return self.handle_prompt_mouse(mouse);
        }

        if let Some(ref mut scroll) = self.help_scroll {
            let max_help = self
                .last_help_lines
                .saturating_sub(self.last_help_height.max(1));
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    *scroll = (*scroll + MOUSE_WHEEL_LINES as u16).min(max_help)
                }
                MouseEventKind::ScrollUp => {
                    *scroll = scroll.saturating_sub(MOUSE_WHEEL_LINES as u16)
                }
                _ => {}
            }
            return false;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(drag) = self.resize_drag_at_mouse(mouse.column, mouse.row) {
                    self.mouse_drag = Some(drag);
                    self.update_dragged_panes(mouse.column, mouse.row);
                    return false;
                }

                match self.mouse_target(mouse.column, mouse.row) {
                    Some(MouseTarget::LeftPane) => {
                        self.focus = FocusPane::Left;
                        self.input_target = InputTarget::None;
                        self.fullscreen_overlay = FullscreenOverlay::None;
                    }
                    Some(MouseTarget::LeftRow(index)) => {
                        let double_click =
                            self.register_mouse_click(MouseClickTarget::LeftPane, Some(index));
                        self.left_section = LeftSection::Projects;
                        self.set_left_selection(index);
                        if double_click {
                            self.activate_selected_left_item_from_mouse();
                        }
                    }
                    Some(MouseTarget::TerminalRow(index)) => {
                        let double_click =
                            self.register_mouse_click(MouseClickTarget::LeftPane, Some(index));
                        self.focus = FocusPane::Left;
                        self.left_section = LeftSection::Terminals;
                        self.selected_terminal_index = index;
                        self.input_target = InputTarget::None;
                        self.fullscreen_overlay = FullscreenOverlay::None;
                        if double_click {
                            let _ = self.open_terminal_from_terminal_list();
                        }
                    }
                    Some(MouseTarget::TerminalPane) => {
                        self.focus = FocusPane::Left;
                        self.left_section = LeftSection::Terminals;
                        self.input_target = InputTarget::None;
                        self.fullscreen_overlay = FullscreenOverlay::None;
                    }
                    Some(MouseTarget::Center) => {
                        let double_click =
                            self.register_mouse_click(MouseClickTarget::CenterPane, None);
                        self.focus = FocusPane::Center;
                        if double_click {
                            self.activate_center_agent_from_mouse();
                        }
                    }
                    Some(MouseTarget::FilesPane) => {
                        self.focus = FocusPane::Files;
                        self.input_target = InputTarget::None;
                        self.fullscreen_overlay = FullscreenOverlay::None;
                    }
                    Some(MouseTarget::UnstagedFile(index)) => {
                        self.set_file_selection(RightSection::Unstaged, index);
                        if index.is_some() {
                            let double_click =
                                self.register_mouse_click(MouseClickTarget::UnstagedPane, index);
                            if double_click {
                                self.open_selected_file_diff_from_mouse();
                            }
                        }
                    }
                    Some(MouseTarget::StagedFile(index)) => {
                        self.set_file_selection(RightSection::Staged, index);
                        if index.is_some() {
                            let double_click =
                                self.register_mouse_click(MouseClickTarget::StagedPane, index);
                            if double_click {
                                self.open_selected_file_diff_from_mouse();
                            }
                        }
                    }
                    Some(MouseTarget::CommitChrome) => {
                        self.set_file_selection(RightSection::CommitInput, None);
                    }
                    Some(MouseTarget::CommitText) => {
                        let ready_to_edit = self.focus == FocusPane::Files
                            && self.right_section == RightSection::CommitInput
                            && self.input_target != InputTarget::CommitMessage;
                        if ready_to_edit || self.input_target == InputTarget::CommitMessage {
                            self.engage_commit_input();
                            self.set_commit_cursor_from_mouse(mouse.column, mouse.row);
                        } else {
                            self.set_file_selection(RightSection::CommitInput, None);
                        }
                    }
                    None => {}
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.mouse_drag.is_some() {
                    self.update_dragged_panes(mouse.column, mouse.row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.mouse_drag.take().is_some() {
                    self.persist_pane_widths();
                }
            }
            MouseEventKind::ScrollDown => match self.mouse_target(mouse.column, mouse.row) {
                Some(MouseTarget::LeftRow(_)) => {
                    self.handle_left_mouse_wheel(true, mouse.column, mouse.row)
                }
                Some(MouseTarget::Center) => self.handle_center_mouse_wheel(mouse),
                Some(MouseTarget::UnstagedFile(_)) => {
                    self.scroll_file_selection(RightSection::Unstaged, true)
                }
                Some(MouseTarget::StagedFile(_)) => {
                    self.scroll_file_selection(RightSection::Staged, true)
                }
                Some(MouseTarget::CommitChrome | MouseTarget::CommitText) => {
                    self.focus = FocusPane::Files;
                    self.right_section = RightSection::CommitInput;
                    self.scroll_commit_input(true);
                }
                _ => {}
            },
            MouseEventKind::ScrollUp => match self.mouse_target(mouse.column, mouse.row) {
                Some(MouseTarget::LeftRow(_)) => {
                    self.handle_left_mouse_wheel(false, mouse.column, mouse.row)
                }
                Some(MouseTarget::Center) => self.handle_center_mouse_wheel(mouse),
                Some(MouseTarget::UnstagedFile(_)) => {
                    self.scroll_file_selection(RightSection::Unstaged, false)
                }
                Some(MouseTarget::StagedFile(_)) => {
                    self.scroll_file_selection(RightSection::Staged, false)
                }
                Some(MouseTarget::CommitChrome | MouseTarget::CommitText) => {
                    self.focus = FocusPane::Files;
                    self.right_section = RightSection::CommitInput;
                    self.scroll_commit_input(false);
                }
                _ => {}
            },
            _ => {}
        }
        false
    }

    // -- Terminal text selection helpers --

    /// Map screen coordinates to terminal grid position.
    /// Returns `None` if the point is outside the terminal area.
    fn screen_to_grid(&self, screen_col: u16, screen_row: u16) -> Option<TermGridPos> {
        let term_area = self.mouse_layout.agent_term?;
        if !contains_point(term_area, screen_col, screen_row) {
            return None;
        }
        Some(TermGridPos {
            row: screen_row - term_area.y,
            col: screen_col - term_area.x,
        })
    }

    /// Map screen coordinates to terminal grid position, clamping to the
    /// terminal area edges. Used during drag so the selection extends to the
    /// nearest boundary when the mouse leaves the terminal area.
    fn screen_to_grid_clamped(&self, screen_col: u16, screen_row: u16) -> Option<TermGridPos> {
        let term_area = self.mouse_layout.agent_term?;
        let col = screen_col
            .max(term_area.x)
            .min(term_area.x + term_area.width.saturating_sub(1))
            - term_area.x;
        let row = screen_row
            .max(term_area.y)
            .min(term_area.y + term_area.height.saturating_sub(1))
            - term_area.y;
        Some(TermGridPos { row, col })
    }

    /// Handle a mouse event for terminal text selection (click, drag, release).
    fn handle_terminal_selection_mouse(&mut self, mouse_ev: MouseEvent) {
        match mouse_ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(pos) = self.screen_to_grid(mouse_ev.column, mouse_ev.row) {
                    self.terminal_selection = Some(TerminalSelection {
                        anchor: pos,
                        end: pos,
                        dragging: true,
                    });
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let pos = self.screen_to_grid_clamped(mouse_ev.column, mouse_ev.row);
                if let Some(sel) = &mut self.terminal_selection
                    && sel.dragging
                    && let Some(pos) = pos
                {
                    sel.end = pos;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(sel) = &mut self.terminal_selection
                    && sel.dragging
                {
                    sel.dragging = false;
                    if sel.anchor == sel.end {
                        // Single click, no actual selection.
                        self.terminal_selection = None;
                    } else {
                        self.copy_terminal_selection();
                    }
                }
            }
            _ => {}
        }
    }

    /// Extract text from the terminal snapshot within the active selection
    /// and copy it to the system clipboard.
    fn copy_terminal_selection(&mut self) {
        let sel = match self.terminal_selection.clone() {
            Some(s) => s,
            None => return,
        };
        let (start, end) = sel.ordered();

        // Refresh snapshot to get current content.
        self.refresh_snapshot_buf();

        let mut lines: Vec<String> = Vec::new();
        let mut current_row = start.row;
        let mut current_line = String::new();

        for cell in &self.snapshot_buf.cells {
            if !sel.contains(cell.row, cell.col) {
                continue;
            }
            if cell.row != current_row {
                // Flush the previous line (trim trailing whitespace).
                lines.push(current_line.trim_end().to_string());
                // Insert empty lines for any gap rows.
                for _ in (current_row + 1)..cell.row {
                    lines.push(String::new());
                }
                current_line = String::new();
                current_row = cell.row;
            }
            // Pad with spaces if columns are not contiguous (sparse cells).
            let expected_col = if current_line.is_empty() {
                start.col.min(cell.col)
            } else {
                // Approximate: one char per column.
                let line_cols = current_line.chars().count() as u16;
                if cell.row == start.row {
                    start.col + line_cols
                } else {
                    line_cols
                }
            };
            if cell.col > expected_col {
                for _ in 0..(cell.col - expected_col) {
                    current_line.push(' ');
                }
            }
            current_line.push_str(&cell.symbol);
        }
        // Flush last line.
        if !current_line.is_empty() || !lines.is_empty() {
            lines.push(current_line.trim_end().to_string());
            // Fill gap rows between last populated row and end.
            for _ in (current_row + 1)..=end.row {
                lines.push(String::new());
            }
        }

        let text = lines.join("\n");
        if !text.is_empty() {
            let _ = self.clipboard.copy_text(
                &text,
                "Terminal text copied to clipboard.",
                &self.worker_tx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, mpsc};

    use super::DOUBLE_CLICK_THRESHOLD;
    use crate::app::{
        App, CenterMode, ConfirmKillRunningPrompt, FocusPane, FullscreenOverlay, InputTarget,
        KillRunningAction, KillRunningFocus, KillRunningFooterAction, KillRunningPrompt,
        KillableRuntime, KillableRuntimeKind, LeftSection, MouseClickTarget, MouseLayoutState,
        OverlayMouseLayout, OverlayMouseLayoutState, PromptState, PullTarget, RightSection,
        RuntimeTargetId, TextInput, WorkerEvent,
    };
    use crate::clipboard::Clipboard;
    use crate::config::{Config, DuxPaths, ProjectConfig};
    use crate::editor::{DetectedEditor, EditorKind};
    use crate::keybindings::{Action, BINDING_DEFS, BindingScope, RuntimeBindings};
    use crate::model::{
        AgentSession, ChangedFile, CompanionTerminalStatus, Project, ProviderKind, SessionStatus,
        SessionSurface,
    };
    use crate::pty::PtyClient;
    use crate::statusline::StatusLine;
    use crate::storage::SessionStore;
    use crate::theme::Theme;
    use chrono::Utc;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use std::process::Command;
    use tempfile::tempdir;

    fn default_bindings() -> RuntimeBindings {
        RuntimeBindings::new(
            |action| {
                BINDING_DEFS
                    .iter()
                    .find(|d| d.action == action)
                    .map(|d| d.default_keys.to_vec())
                    .unwrap_or_default()
            },
            true,
        )
    }

    fn bindings_with_overrides(overrides: &[(Action, &[&str])]) -> RuntimeBindings {
        RuntimeBindings::new(
            |action| {
                if let Some((_, keys)) =
                    overrides.iter().find(|(candidate, _)| *candidate == action)
                {
                    return keys
                        .iter()
                        .map(|key| crokey::parse(key).expect("valid test binding"))
                        .collect();
                }
                BINDING_DEFS
                    .iter()
                    .find(|d| d.action == action)
                    .map(|d| d.default_keys.to_vec())
                    .unwrap_or_default()
            },
            true,
        )
    }

    fn test_app(bindings: RuntimeBindings) -> App {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);

        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            root: root.clone(),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let session_store = SessionStore::open(&paths.sessions_db_path).expect("session store");
        let now = Utc::now();
        let project = Project {
            id: "project-1".to_string(),
            name: "demo".to_string(),
            path: root.to_string_lossy().to_string(),
            default_provider: ProviderKind::from_str("codex"),
            current_branch: "main".to_string(),
        };
        let session = AgentSession {
            id: "session-1".to_string(),
            project_id: project.id.clone(),
            project_path: Some(project.path.clone()),
            provider: ProviderKind::from_str("codex"),
            source_branch: "main".to_string(),
            branch_name: "agent-branch".to_string(),
            worktree_path: paths.worktrees_root.to_string_lossy().to_string(),
            title: None,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
        };
        let (worker_tx, worker_rx) = mpsc::channel();
        let mut app = App {
            config: Config::default(),
            paths,
            bindings,
            session_store,
            projects: vec![project],
            sessions: vec![session],
            staged_files: Vec::new(),
            unstaged_files: Vec::new(),
            selected_left: 0,
            left_section: crate::app::LeftSection::Projects,
            selected_terminal_index: 0,
            right_section: RightSection::Unstaged,
            files_index: 0,
            files_search: TextInput::new(),
            files_search_active: false,
            commit_input: TextInput::new()
                .with_multiline(4)
                .with_placeholder("Type your commit message\u{2026}"),
            show_diff_line_numbers: false,
            left_width_pct: 20,
            right_width_pct: 23,
            terminal_pane_height_pct: 35,
            staged_pane_height_pct: 50,
            commit_pane_height_pct: 40,
            focus: FocusPane::Left,
            center_mode: CenterMode::Agent,
            left_collapsed: false,
            right_collapsed: false,
            right_hidden: false,
            resize_mode: false,
            help_scroll: None,
            last_help_height: 0,
            last_help_lines: 0,
            fullscreen_overlay: FullscreenOverlay::None,
            status: StatusLine::new("ready"),
            prompt: PromptState::None,
            input_target: InputTarget::None,
            session_surface: crate::model::SessionSurface::Agent,
            clipboard: Clipboard::new(),
            worker_tx,
            worker_rx,
            providers: std::collections::HashMap::new(),
            companion_terminals: std::collections::HashMap::new(),
            active_terminal_id: None,
            terminal_return_to_list: false,
            terminal_counter: 0,
            create_agent_in_flight: false,
            pulls_in_flight: std::collections::HashSet::new(),
            resource_stats_in_flight: false,
            last_pty_size: (0, 0),
            last_pty_activity: std::collections::HashMap::new(),
            prev_scrollback_offset: 0,
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme: Theme::default_dark(),
            tick_count: 0,
            start_time: std::time::Instant::now(),
            readonly_nudge_tick: None,
            watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
            has_active_processes: Arc::new(AtomicBool::new(false)),
            collapsed_projects: std::collections::HashSet::new(),
            left_items_cache: Vec::new(),
            mouse_layout: MouseLayoutState::default(),
            overlay_layout: OverlayMouseLayoutState::default(),
            mouse_drag: None,
            last_mouse_click: None,
            interactive_patterns: crate::keybindings::InteractiveBytePatterns {
                bindings: Vec::new(),
            },
            raw_input_buf: Vec::new(),
            macro_bar: None,
            sigwinch_flag: Arc::new(AtomicBool::new(false)),
            force_redraw: false,
            welcome_tip_index: 0,
            welcome_logo_visible: false,
            welcome_tip_selection: usize::MAX,
            branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            gh_status: crate::model::GhStatus::Unknown,
            github_integration_enabled: false,
            pr_banner_at_bottom: true,
            pr_statuses: std::collections::HashMap::new(),
            pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync_enabled: Arc::new(AtomicBool::new(false)),
            pr_last_checked: std::collections::HashMap::new(),
            refs_watcher: None,
            refs_watch_paths: std::collections::HashMap::new(),
            resume_fallback_candidates: std::collections::HashSet::new(),
            syntax_cache: crate::diff::SyntaxCache::new(),
            snapshot_buf: crate::pty::TerminalSnapshot::empty(),
            last_snapshot_id: None,
            terminal_selection: None,
        };
        app.interactive_patterns = app.bindings.interactive_byte_patterns();
        app.rebuild_left_items();
        app.selected_left = 1;
        app
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn install_mouse_layout(app: &mut App) {
        app.mouse_layout = MouseLayoutState {
            body: Rect::new(0, 0, 100, 20),
            left: Rect::new(0, 0, 20, 20),
            center: Rect::new(20, 0, 57, 20),
            right: Rect::new(77, 0, 23, 20),
            left_list: Rect::new(1, 1, 18, 10),
            terminal_list: Rect::default(),
            agent_term: Some(Rect::new(21, 1, 55, 16)),
            unstaged_list: Some(Rect::new(78, 1, 21, 8)),
            staged_list: Some(Rect::new(78, 9, 21, 5)),
            commit_area: Some(Rect::new(77, 14, 23, 6)),
            commit_text_area: Some(Rect::new(78, 15, 21, 4)),
        };
    }

    fn install_command_overlay(app: &mut App, items: usize) {
        app.overlay_layout.active = OverlayMouseLayout::Command {
            input: Rect::new(15, 7, 70, 1),
            list: Rect::new(15, 9, 70, 6),
            items,
            offset: 0,
        };
    }

    fn install_browser_overlay(app: &mut App, items: usize) {
        app.overlay_layout.active = OverlayMouseLayout::BrowseProjects {
            input: Some(Rect::new(15, 4, 70, 1)),
            list: Rect::new(15, 6, 70, 8),
            items,
            offset: 0,
        };
    }

    fn install_pick_editor_overlay(app: &mut App, items: usize) {
        app.overlay_layout.active = OverlayMouseLayout::PickEditor {
            list: Rect::new(19, 8, 62, 6),
            items,
            offset: 0,
        };
    }

    fn install_kill_running_overlay(app: &mut App, items: usize) {
        app.overlay_layout.active = OverlayMouseLayout::KillRunning {
            input: Some(Rect::new(12, 4, 70, 1)),
            list: Rect::new(12, 6, 70, 8),
            items,
            offset: 0,
            cancel_button: Rect::new(12, 16, 14, 3),
            hovered_button: Rect::new(28, 16, 16, 3),
            selected_button: Rect::new(46, 16, 17, 3),
            visible_button: Rect::new(65, 16, 15, 3),
        };
    }

    fn install_confirm_delete_overlay(app: &mut App) {
        app.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteAgent {
            cancel_button: Rect::new(34, 10, 16, 3),
            delete_button: Rect::new(52, 10, 16, 3),
        };
    }

    fn install_confirm_quit_overlay(app: &mut App) {
        app.overlay_layout.active = OverlayMouseLayout::ConfirmQuit {
            cancel_button: Rect::new(34, 10, 16, 3),
            quit_button: Rect::new(52, 10, 16, 3),
        };
    }

    fn install_confirm_discard_overlay(app: &mut App) {
        app.overlay_layout.active = OverlayMouseLayout::ConfirmDiscardFile {
            cancel_button: Rect::new(34, 10, 16, 3),
            discard_button: Rect::new(52, 10, 16, 3),
        };
    }

    fn install_rename_overlay(app: &mut App) {
        app.overlay_layout.active = OverlayMouseLayout::RenameSession {
            input: Rect::new(24, 10, 30, 1),
        };
    }

    fn clipboard_ok(_: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn clipboard_fail(_: &str) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("linux clipboard unavailable"))
    }

    fn sample_runtime(
        id: RuntimeTargetId,
        kind: KillableRuntimeKind,
        label: &str,
        context: &str,
    ) -> KillableRuntime {
        KillableRuntime {
            id,
            kind,
            label: label.to_string(),
            context: context.to_string(),
            search_text: format!("{label} {context}"),
        }
    }

    fn init_git_repo_with_modified_file(
        app: &App,
        relative_path: &str,
        original: &str,
        updated: &str,
    ) {
        let worktree = std::path::Path::new(&app.sessions[0].worktree_path);
        std::fs::create_dir_all(worktree).expect("worktree dir");

        Command::new("git")
            .args(["init"])
            .current_dir(worktree)
            .output()
            .expect("git init");
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(worktree)
            .output()
            .expect("git email");
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(worktree)
            .output()
            .expect("git name");

        let file_path = worktree.join(relative_path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).expect("file parent");
        }
        std::fs::write(&file_path, original).expect("write original");
        Command::new("git")
            .args(["add", relative_path])
            .current_dir(worktree)
            .output()
            .expect("git add");
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(worktree)
            .output()
            .expect("git commit");

        std::fs::write(&file_path, updated).expect("write updated");
    }

    #[test]
    fn refresh_selected_project_blocks_repeat_presses_while_pull_is_running() {
        let mut app = test_app(default_bindings());
        app.selected_left = 0;

        app.refresh_selected_project()
            .expect("start project refresh");
        app.refresh_selected_project()
            .expect("repeat refresh should not error");

        let repo_path = app.projects[0].path.clone();
        assert!(app.pulls_in_flight.contains(&repo_path));
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Warning);
        assert!(app.status.text().contains("already in progress"));
    }

    #[test]
    fn project_pull_completion_clears_in_flight_guard() {
        let mut app = test_app(default_bindings());
        let repo_path = app.projects[0].path.clone();
        app.pulls_in_flight.insert(repo_path.clone());

        app.worker_tx
            .send(WorkerEvent::PullCompleted {
                repo_path,
                target: PullTarget::Project {
                    project_id: app.projects[0].id.clone(),
                    project_name: app.projects[0].name.clone(),
                },
                result: Ok(Some("feature/demo".to_string())),
            })
            .expect("queue worker event");

        app.drain_events();

        assert!(app.pulls_in_flight.is_empty());
        assert_eq!(app.projects[0].current_branch, "feature/demo");
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Info);
    }

    #[test]
    fn pull_from_remote_blocks_repeat_presses_while_pull_is_running() {
        let mut app = test_app(default_bindings());
        app.selected_left = 1;

        app.pull_from_remote().expect("start session pull");
        app.pull_from_remote()
            .expect("repeat pull should not error");

        let repo_path = app.sessions[0].worktree_path.clone();
        assert!(app.pulls_in_flight.contains(&repo_path));
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Warning);
        assert!(app.status.text().contains("already in progress"));
    }

    #[test]
    fn rename_session_prompt_accepts_text_before_agent_input() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: TextInput::with_text("agent".to_string()),
            rename_branch: false,
        };
        app.input_target = InputTarget::Agent;

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::RenameSession { input, .. } => {
                assert_eq!(input.text, "agentx");
                assert_eq!(input.cursor, 6);
            }
            other => panic!("expected rename prompt, got {other:?}"),
        }
    }

    #[test]
    fn rename_session_text_ignores_printable_close_overlay_binding() {
        let mut app = test_app(bindings_with_overrides(&[(Action::CloseOverlay, &["x"])]));
        app.prompt = PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: TextInput::with_text("agent".to_string()),
            rename_branch: false,
        };

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::RenameSession { input, .. } => {
                assert_eq!(input.text, "agentx");
                assert_eq!(input.cursor, 6);
            }
            other => panic!("expected rename prompt, got {other:?}"),
        }
    }

    #[test]
    fn rename_session_uses_custom_dialog_confirm_binding() {
        let mut app = test_app(bindings_with_overrides(&[(Action::Confirm, &["tab"])]));
        app.prompt = PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: TextInput::with_text("agent-branch".to_string()),
            rename_branch: false,
        };

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(
            app.sessions[0].title.as_deref(),
            Some("agent-branch"),
            "custom confirm binding should apply the rename"
        );
    }

    #[test]
    fn open_rename_session_clears_interactive_target() {
        let mut app = test_app(default_bindings());
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;

        app.open_rename_session().unwrap();

        assert!(matches!(app.prompt, PromptState::RenameSession { .. }));
        assert_eq!(app.input_target, InputTarget::None);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
    }

    #[test]
    fn rename_title_only_does_not_change_branch_name() {
        let mut app = test_app(default_bindings());
        let original_branch = app.sessions[0].branch_name.clone();

        app.apply_rename_session("session-1", "new-title".to_string(), false);

        assert_eq!(
            app.sessions[0].title.as_deref(),
            Some("new-title"),
            "title should be updated"
        );
        assert_eq!(
            app.sessions[0].branch_name, original_branch,
            "branch_name should remain unchanged when rename_branch is false"
        );
    }

    #[test]
    fn rename_toggle_checkbox_flips_rename_branch() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: TextInput::with_text("test".to_string()),
            rename_branch: true,
        };

        // Tab toggles the checkbox.
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::RenameSession { rename_branch, .. } => {
                assert!(!*rename_branch, "Tab should toggle rename_branch to false");
            }
            other => panic!("expected RenameSession, got {other:?}"),
        }

        // Tab again toggles it back.
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::RenameSession { rename_branch, .. } => {
                assert!(
                    *rename_branch,
                    "second Tab should toggle rename_branch back to true"
                );
            }
            other => panic!("expected RenameSession, got {other:?}"),
        }
    }

    #[test]
    fn open_rename_session_initializes_rename_branch_true() {
        let mut app = test_app(default_bindings());

        app.open_rename_session().unwrap();

        match &app.prompt {
            PromptState::RenameSession { rename_branch, .. } => {
                assert!(*rename_branch, "rename_branch should default to true");
            }
            other => panic!("expected RenameSession, got {other:?}"),
        }
    }

    #[test]
    fn open_kill_running_requires_live_runtimes() {
        let mut app = test_app(default_bindings());

        app.open_kill_running().unwrap();

        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Error);
        assert!(app.status.text().contains("No running agents"));
    }

    #[test]
    fn open_kill_running_snapshots_agents_and_terminals() {
        let mut app = test_app(default_bindings());
        let worktree_path = app.sessions[0].worktree_path.clone();
        let worktree = std::path::Path::new(&worktree_path);
        let args = vec!["-c".to_string(), "sleep 5".to_string()];
        app.providers.insert(
            app.sessions[0].id.clone(),
            PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000).expect("spawn agent"),
        );
        app.companion_terminals.insert(
            "term-1".to_string(),
            crate::app::CompanionTerminal {
                session_id: app.sessions[0].id.clone(),
                label: "shell".to_string(),
                foreground_cmd: Some("python".to_string()),
                client: PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000)
                    .expect("spawn terminal"),
            },
        );

        app.open_kill_running().unwrap();

        match &app.prompt {
            PromptState::KillRunning(prompt) => {
                assert_eq!(prompt.runtimes.len(), 2);
                let agent = prompt
                    .runtimes
                    .iter()
                    .find(|runtime| matches!(runtime.id, RuntimeTargetId::Agent(_)))
                    .expect("agent runtime");
                assert_eq!(agent.label, "Codex");
                assert_eq!(
                    agent.context,
                    "on agent \"agent-branch\" under project \"demo\""
                );

                let terminal = prompt
                    .runtimes
                    .iter()
                    .find(|runtime| matches!(runtime.id, RuntimeTargetId::Terminal(_)))
                    .expect("terminal runtime");
                assert_eq!(terminal.label, "python");
                assert_eq!(
                    terminal.context,
                    "on agent \"agent-branch\" under project \"demo\""
                );
            }
            other => panic!("expected kill-running prompt, got {other:?}"),
        }
    }

    #[test]
    fn kill_running_terminal_label_deduplicates_term_prefix() {
        let mut app = test_app(default_bindings());
        let worktree_path = app.sessions[0].worktree_path.clone();
        let worktree = std::path::Path::new(&worktree_path);
        let args = vec!["-c".to_string(), "sleep 5".to_string()];
        app.companion_terminals.insert(
            "term-1".to_string(),
            crate::app::CompanionTerminal {
                session_id: app.sessions[0].id.clone(),
                label: "shell".to_string(),
                foreground_cmd: Some("TERM sleep".to_string()),
                client: PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000)
                    .expect("spawn terminal"),
            },
        );

        let runtimes = app.running_runtime_snapshot();
        let terminal = runtimes
            .iter()
            .find(|runtime| matches!(runtime.id, RuntimeTargetId::Terminal(_)))
            .expect("terminal runtime");
        assert_eq!(terminal.label, "sleep");
    }

    #[test]
    fn kill_running_tab_focus_reaches_cancel_button() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes: vec![sample_runtime(
                RuntimeTargetId::Agent("session-1".to_string()),
                KillableRuntimeKind::Agent,
                "Codex",
                "on agent \"agent-branch\" under project \"demo\"",
            )],
            filter: TextInput::new(),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: std::collections::HashSet::new(),
            focus: KillRunningFocus::List,
        });

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::KillRunning(prompt) => {
                assert_eq!(
                    prompt.focus,
                    KillRunningFocus::Footer(KillRunningFooterAction::Cancel)
                )
            }
            other => panic!("expected kill-running prompt, got {other:?}"),
        }
    }

    #[test]
    fn kill_running_footer_skips_kill_selected_when_nothing_is_marked() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes: vec![sample_runtime(
                RuntimeTargetId::Agent("session-1".to_string()),
                KillableRuntimeKind::Agent,
                "Codex",
                "on agent \"agent-branch\" under project \"demo\"",
            )],
            filter: TextInput::new(),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: std::collections::HashSet::new(),
            focus: KillRunningFocus::Footer(KillRunningFooterAction::Hovered),
        });

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::KillRunning(prompt) => {
                assert_eq!(
                    prompt.focus,
                    KillRunningFocus::Footer(KillRunningFooterAction::Visible)
                )
            }
            other => panic!("expected kill-running prompt, got {other:?}"),
        }
    }

    #[test]
    fn kill_running_search_keeps_hidden_selection() {
        let mut app = test_app(default_bindings());
        let selected_id = RuntimeTargetId::Agent("session-1".to_string());
        app.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes: vec![
                sample_runtime(
                    selected_id.clone(),
                    KillableRuntimeKind::Agent,
                    "alpha-agent",
                    "demo / codex / alpha-agent",
                ),
                sample_runtime(
                    RuntimeTargetId::Terminal("term-1".to_string()),
                    KillableRuntimeKind::Terminal,
                    "beta-shell",
                    "demo / alpha-agent",
                ),
            ],
            filter: TextInput::new(),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: std::iter::once(selected_id.clone()).collect(),
            focus: KillRunningFocus::List,
        });

        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .unwrap();
        for ch in ['b', 'e', 't', 'a'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))
                .unwrap();
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        let PromptState::KillRunning(prompt) = &app.prompt else {
            panic!("expected kill-running prompt");
        };
        let visible = App::visible_kill_running_indices(prompt);
        assert_eq!(visible.len(), 1);
        assert!(prompt.selected_ids.contains(&selected_id));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        let PromptState::KillRunning(prompt) = &app.prompt else {
            panic!("expected kill-running prompt");
        };
        assert!(prompt.selected_ids.contains(&selected_id));
    }

    #[test]
    fn confirm_kill_cancel_restores_selection_modal_state() {
        let mut app = test_app(default_bindings());
        let selected_id = RuntimeTargetId::Agent("session-1".to_string());
        let previous = KillRunningPrompt {
            runtimes: vec![sample_runtime(
                selected_id.clone(),
                KillableRuntimeKind::Agent,
                "agent-branch",
                "demo / codex / agent-branch",
            )],
            filter: TextInput::with_text("agent".to_string()),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: std::iter::once(selected_id).collect(),
            focus: KillRunningFocus::Footer(KillRunningFooterAction::Selected),
        };
        app.prompt = PromptState::ConfirmKillRunning(ConfirmKillRunningPrompt {
            previous: previous.clone(),
            action: KillRunningAction::Selected,
            target_ids: vec![RuntimeTargetId::Agent("session-1".to_string())],
            confirm_selected: false,
        });

        app.resolve_confirm_kill_running(false);

        match &app.prompt {
            PromptState::KillRunning(prompt) => {
                assert_eq!(prompt.filter.text, previous.filter.text);
                assert_eq!(prompt.selected_ids, previous.selected_ids);
                assert_eq!(prompt.focus, previous.focus);
            }
            other => panic!("expected restored kill-running prompt, got {other:?}"),
        }
    }

    #[test]
    fn kill_visible_only_kills_filtered_rows() {
        let mut app = test_app(default_bindings());
        let worktree_path = app.sessions[0].worktree_path.clone();
        let worktree = std::path::Path::new(&worktree_path);
        std::fs::create_dir_all(app.paths.worktrees_root.join("other")).expect("other worktree");
        let now = Utc::now();
        app.sessions.push(AgentSession {
            id: "session-2".to_string(),
            project_id: app.projects[0].id.clone(),
            project_path: Some(app.projects[0].path.clone()),
            provider: ProviderKind::from_str("claude"),
            source_branch: "main".to_string(),
            branch_name: "beta-agent".to_string(),
            worktree_path: app.paths.worktrees_root.join("other").display().to_string(),
            title: None,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
        });
        let args = vec!["-c".to_string(), "sleep 5".to_string()];
        app.providers.insert(
            "session-1".to_string(),
            PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000).expect("spawn first"),
        );
        app.providers.insert(
            "session-2".to_string(),
            PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000).expect("spawn second"),
        );

        app.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes: app.running_runtime_snapshot(),
            filter: TextInput::with_text("beta".to_string()),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: std::iter::once(RuntimeTargetId::Agent("session-1".to_string()))
                .collect(),
            focus: KillRunningFocus::Footer(KillRunningFooterAction::Visible),
        });

        app.open_confirm_kill_running_action(KillRunningAction::Visible)
            .unwrap();
        app.resolve_confirm_kill_running(true);

        assert!(app.providers.contains_key("session-1"));
        assert!(!app.providers.contains_key("session-2"));
    }

    #[test]
    fn kill_selected_removes_running_targets_and_resets_terminal_surface() {
        let mut app = test_app(default_bindings());
        let worktree = std::path::Path::new(&app.sessions[0].worktree_path);
        let args = vec!["-c".to_string(), "sleep 5".to_string()];
        app.providers.insert(
            "session-1".to_string(),
            PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000).expect("spawn agent"),
        );
        app.companion_terminals.insert(
            "term-1".to_string(),
            crate::app::CompanionTerminal {
                session_id: app.sessions[0].id.clone(),
                label: "shell".to_string(),
                foreground_cmd: None,
                client: PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000)
                    .expect("spawn terminal"),
            },
        );
        app.active_terminal_id = Some("term-1".to_string());
        app.session_surface = SessionSurface::Terminal;
        app.input_target = InputTarget::None;
        app.fullscreen_overlay = FullscreenOverlay::Terminal;

        app.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes: app.running_runtime_snapshot(),
            filter: TextInput::new(),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: [
                RuntimeTargetId::Agent("session-1".to_string()),
                RuntimeTargetId::Terminal("term-1".to_string()),
            ]
            .into_iter()
            .collect(),
            focus: KillRunningFocus::Footer(KillRunningFooterAction::Selected),
        });

        app.open_confirm_kill_running_action(KillRunningAction::Selected)
            .unwrap();
        app.resolve_confirm_kill_running(true);

        assert!(app.providers.is_empty());
        assert!(app.companion_terminals.is_empty());
        assert_eq!(app.session_surface, SessionSurface::Agent);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
        assert!(matches!(app.prompt, PromptState::None));
    }

    #[test]
    fn kill_selected_warns_when_targets_are_already_gone() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ConfirmKillRunning(ConfirmKillRunningPrompt {
            previous: KillRunningPrompt {
                runtimes: vec![sample_runtime(
                    RuntimeTargetId::Agent("session-1".to_string()),
                    KillableRuntimeKind::Agent,
                    "agent-branch",
                    "demo / codex / agent-branch",
                )],
                filter: TextInput::new(),
                searching: false,
                hovered_visible_index: 0,
                selected_ids: std::iter::once(RuntimeTargetId::Agent("session-1".to_string()))
                    .collect(),
                focus: KillRunningFocus::Footer(KillRunningFooterAction::Selected),
            },
            action: KillRunningAction::Selected,
            target_ids: vec![RuntimeTargetId::Agent("session-1".to_string())],
            confirm_selected: true,
        });

        app.resolve_confirm_kill_running(true);

        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Warning);
        assert!(app.status.text().contains("already gone"));
    }

    #[test]
    fn fork_selected_session_sets_busy_state_and_keeps_fresh_create_flow() {
        let mut app = test_app(default_bindings());

        app.fork_selected_session().unwrap();

        assert!(app.create_agent_in_flight);
        assert!(app.status.text().contains("Forking agent"));
        assert!(app.status.text().contains("fresh session"));
    }

    #[test]
    fn fork_action_requires_selected_session() {
        let mut app = test_app(default_bindings());
        app.selected_left = 0;

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
            .unwrap();

        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Error);
        assert!(
            app.status
                .text()
                .contains("Select an agent session first to fork.")
        );
    }

    #[test]
    fn copy_path_copies_selected_session_worktree() {
        let mut app = test_app(default_bindings());
        let worktree_path = app.sessions[0].worktree_path.clone();
        app.clipboard = Clipboard::from_fn(clipboard_ok);

        app.copy_selected_path().unwrap();
        // Result arrives via WorkerEvent; drain it.
        std::thread::sleep(std::time::Duration::from_millis(50));
        app.drain_events();

        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Info);
        assert_eq!(app.status.text(), "Agent's path copied to clipboard.");
    }

    #[test]
    fn copy_path_copies_selected_project_path() {
        let mut app = test_app(default_bindings());
        app.selected_left = 0;
        app.clipboard = Clipboard::from_fn(clipboard_ok);

        app.copy_selected_path().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        app.drain_events();

        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Info);
        assert_eq!(app.status.text(), "Agent's path copied to clipboard.");
    }

    #[test]
    fn copy_path_requires_project_or_session_selection() {
        let mut app = test_app(default_bindings());
        app.left_items_cache.clear();
        app.selected_left = 0;

        app.copy_selected_path().unwrap();

        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Error);
        assert!(
            app.status
                .text()
                .contains("No project or agent selected. Select one from the sidebar first.")
        );
    }

    #[test]
    fn copy_path_failure_sets_error_status() {
        let mut app = test_app(default_bindings());
        app.clipboard = Clipboard::from_fn(clipboard_fail);

        app.copy_selected_path().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        app.drain_events();

        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Error);
        assert!(app.status.text().contains("Clipboard copy failed"));
        assert!(app.status.text().contains("linux clipboard unavailable"));
    }

    #[test]
    fn scroll_page_up_resolves_in_interactive_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Interactive),
            Some(Action::ScrollPageUp),
        );
    }

    #[test]
    fn scroll_page_down_resolves_in_interactive_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Interactive),
            Some(Action::ScrollPageDown),
        );
    }

    #[test]
    fn scroll_line_down_resolves_space_in_interactive_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Interactive),
            Some(Action::ScrollLineDown),
        );
    }

    #[test]
    fn ctrl_b_does_not_resolve_scroll_in_center_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_ne!(
            bindings.lookup(&key, BindingScope::Center),
            Some(Action::ScrollPageUp),
        );
    }

    #[test]
    fn ctrl_f_does_not_resolve_scroll_in_center_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_ne!(
            bindings.lookup(&key, BindingScope::Center),
            Some(Action::ScrollPageDown),
        );
    }

    #[test]
    fn scroll_line_up_resolves_arrow_up_in_interactive_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Interactive),
            Some(Action::ScrollLineUp),
        );
    }

    #[test]
    fn scroll_line_down_resolves_arrow_down_in_interactive_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Interactive),
            Some(Action::ScrollLineDown),
        );
    }

    #[test]
    fn scroll_line_up_resolves_arrow_up_in_center_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Center),
            Some(Action::ScrollLineUp),
        );
    }

    #[test]
    fn scroll_line_down_resolves_arrow_down_in_center_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Center),
            Some(Action::ScrollLineDown),
        );
    }

    #[test]
    fn scroll_line_down_resolves_space_in_center_scope() {
        let bindings = default_bindings();
        let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(
            bindings.lookup(&key, BindingScope::Center),
            Some(Action::ScrollLineDown),
        );
    }

    #[test]
    fn slash_search_selects_first_match_and_n_advances() {
        let mut app = test_app(default_bindings());
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;
        app.unstaged_files = vec![
            ChangedFile {
                path: "src/lib.rs".into(),
                status: "M".into(),
                additions: 1,
                deletions: 0,
                binary: false,
            },
            ChangedFile {
                path: "src/main.rs".into(),
                status: "M".into(),
                additions: 2,
                deletions: 1,
                binary: false,
            },
        ];
        app.staged_files = vec![ChangedFile {
            path: "tests/main.rs".into(),
            status: "A".into(),
            additions: 3,
            deletions: 0,
            binary: false,
        }];

        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .unwrap();
        for ch in ['m', 'a', 'i', 'n'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE))
                .unwrap();
        }

        assert!(app.files_search_active);
        assert_eq!(app.files_search.text, "main");
        assert_eq!(app.right_section, RightSection::Unstaged);
        assert_eq!(app.files_index, 1);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();

        assert_eq!(app.right_section, RightSection::Staged);
        assert_eq!(app.files_index, 0);

        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();

        assert_eq!(app.right_section, RightSection::Unstaged);
        assert_eq!(app.files_index, 1);
    }

    #[test]
    fn enter_opens_selected_file_diff_from_files_pane() {
        let mut app = test_app(default_bindings());
        init_git_repo_with_modified_file(
            &app,
            "src/main.rs",
            "fn main() {}\n",
            "fn main() { println!(\"hi\"); }\n",
        );
        app.unstaged_files = vec![ChangedFile {
            path: "src/main.rs".into(),
            status: "M".into(),
            additions: 1,
            deletions: 1,
            binary: false,
        }];
        app.selected_left = 1;
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert_eq!(app.focus, FocusPane::Center);
        assert!(matches!(app.center_mode, CenterMode::Diff { .. }));
    }

    #[test]
    fn mouse_click_left_row_focuses_and_selects_it() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.focus = FocusPane::Center;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1));

        assert_eq!(app.focus, FocusPane::Left);
        assert_eq!(app.selected_left, 0);
    }

    #[test]
    fn mouse_click_left_pane_chrome_focuses_left_pane() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.focus = FocusPane::Files;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));

        assert_eq!(app.focus, FocusPane::Left);
    }

    #[test]
    fn mouse_click_empty_left_list_focuses_left_pane() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.projects.clear();
        app.sessions.clear();
        app.left_items_cache.clear();
        app.selected_left = 0;
        app.focus = FocusPane::Center;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1));

        assert_eq!(app.focus, FocusPane::Left);
    }

    #[test]
    fn mouse_click_right_row_focuses_and_selects_unstaged_file() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.unstaged_files = vec![
            ChangedFile {
                path: "a.txt".into(),
                status: "M".into(),
                additions: 1,
                deletions: 0,
                binary: false,
            },
            ChangedFile {
                path: "b.txt".into(),
                status: "M".into(),
                additions: 2,
                deletions: 1,
                binary: false,
            },
        ];
        app.focus = FocusPane::Center;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 2));

        assert_eq!(app.focus, FocusPane::Files);
        assert_eq!(app.right_section, RightSection::Unstaged);
        assert_eq!(app.files_index, 1);
        assert!(matches!(app.center_mode, CenterMode::Agent));
    }

    #[test]
    fn mouse_click_right_pane_chrome_focuses_files_pane() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.focus = FocusPane::Left;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 99, 0));

        assert_eq!(app.focus, FocusPane::Files);
    }

    #[test]
    fn mouse_journey_can_switch_focus_across_all_panes() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 99, 0));
        assert_eq!(app.focus, FocusPane::Files);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
        assert_eq!(app.focus, FocusPane::Center);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        assert_eq!(app.focus, FocusPane::Left);
    }

    #[test]
    fn mouse_double_click_project_row_activates_like_enter() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.selected_left = 0;
        app.focus = FocusPane::Left;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1));

        // Double-clicking a project activates it (opens latest session).
        assert_eq!(app.focus, FocusPane::Center);
        assert!(matches!(app.center_mode, CenterMode::Agent));
    }

    #[test]
    fn mouse_double_click_session_row_activates_like_enter() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.selected_left = 1;
        app.focus = FocusPane::Left;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 2));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 2));

        assert_eq!(app.focus, FocusPane::Center);
        assert!(matches!(app.center_mode, CenterMode::Agent));
        assert_eq!(app.selected_left, 1);
    }

    #[test]
    fn mouse_double_click_left_pane_empty_space_does_not_activate_selected_row() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.selected_left = 1;
        app.focus = FocusPane::Center;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 9));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 9));

        assert_eq!(app.focus, FocusPane::Left);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
        assert_eq!(app.input_target, InputTarget::None);
        assert!(matches!(app.center_mode, CenterMode::Agent));
        assert_eq!(app.selected_left, 1);
    }

    #[test]
    fn mouse_double_click_center_agent_pane_opens_fullscreen() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.selected_left = 1;
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        app.providers.insert(
            "session-1".to_string(),
            PtyClient::spawn(
                "sh",
                &["-c".to_string(), "printf ready; sleep 0.2".to_string()],
                std::path::Path::new("."),
                10,
                10,
                100,
            )
            .expect("spawn pty"),
        );

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));

        assert_eq!(app.focus, FocusPane::Center);
        assert_eq!(app.input_target, InputTarget::Agent);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Agent);
    }

    #[test]
    fn mouse_double_click_center_from_left_pane_opens_fullscreen() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.selected_left = 1;
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Left;
        app.providers.insert(
            "session-1".to_string(),
            PtyClient::spawn(
                "sh",
                &["-c".to_string(), "printf ready; sleep 0.2".to_string()],
                std::path::Path::new("."),
                10,
                10,
                100,
            )
            .expect("spawn pty"),
        );

        // First click focuses the center pane (from Left).
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
        assert_eq!(app.focus, FocusPane::Center);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);

        // Second click completes the double-click and activates fullscreen.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
        assert_eq!(app.input_target, InputTarget::Agent);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Agent);
    }

    #[test]
    fn mouse_up_between_clicks_does_not_break_double_click() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.selected_left = 1;
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        app.providers.insert(
            "session-1".to_string(),
            PtyClient::spawn(
                "sh",
                &["-c".to_string(), "printf ready; sleep 0.2".to_string()],
                std::path::Path::new("."),
                10,
                10,
                100,
            )
            .expect("spawn pty"),
        );

        // Down → Up → Down mirrors what the event loop sees for a real
        // double-click.  The Up must not interfere with detection.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 30, 5));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));

        assert_eq!(app.input_target, InputTarget::Agent);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Agent);
    }

    #[test]
    fn mouse_wheel_left_pane_advances_selection_under_cursor() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.focus = FocusPane::Center;
        app.selected_left = 0;

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 2, 1));

        assert_eq!(app.focus, FocusPane::Left);
        assert_eq!(app.selected_left, 1);
    }

    #[test]
    fn mouse_journey_can_switch_files_sections_by_clicking_rows() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.unstaged_files = vec![ChangedFile {
            path: "a.txt".into(),
            status: "M".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        }];
        app.staged_files = vec![ChangedFile {
            path: "b.txt".into(),
            status: "A".into(),
            additions: 3,
            deletions: 0,
            binary: false,
        }];

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 1));
        assert_eq!(app.right_section, RightSection::Unstaged);
        assert_eq!(app.files_index, 0);
        assert_eq!(app.focus, FocusPane::Files);
        assert!(matches!(app.center_mode, CenterMode::Agent));

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 9));
        assert_eq!(app.right_section, RightSection::Staged);
        assert_eq!(app.files_index, 0);
        assert_eq!(app.focus, FocusPane::Files);
    }

    #[test]
    fn mouse_double_click_unstaged_file_row_opens_diff() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        init_git_repo_with_modified_file(
            &app,
            "src/main.rs",
            "fn main() {}\n",
            "fn main() { println!(\"hi\"); }\n",
        );
        app.unstaged_files = vec![ChangedFile {
            path: "src/main.rs".into(),
            status: "M".into(),
            additions: 1,
            deletions: 1,
            binary: false,
        }];
        app.selected_left = 1;
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 1));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 1));

        assert_eq!(app.focus, FocusPane::Center);
        assert!(matches!(app.center_mode, CenterMode::Diff { .. }));
    }

    /// Regression test: double-clicking a file when focus starts on Center
    /// must open the diff even though focus changes (and the mouse layout
    /// shifts due to the hint bar appearing) between the two clicks.
    #[test]
    fn mouse_double_click_unstaged_file_opens_diff_from_center_focus() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        init_git_repo_with_modified_file(
            &app,
            "src/main.rs",
            "fn main() {}\n",
            "fn main() { println!(\"hi\"); }\n",
        );
        app.unstaged_files = vec![ChangedFile {
            path: "src/main.rs".into(),
            status: "M".into(),
            additions: 1,
            deletions: 1,
            binary: false,
        }];
        app.selected_left = 1;

        // Focus starts on the center pane (agent output), NOT on Files.
        app.focus = FocusPane::Center;
        app.right_section = RightSection::Unstaged;

        // Layout before first click: full inner area (no hint bar because
        // the pane is not focused).  unstaged_list rows 1..19 (height 18).
        app.mouse_layout.unstaged_list = Some(Rect::new(78, 1, 21, 18));

        // First click selects the file and moves focus to Files.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 1));
        assert_eq!(app.focus, FocusPane::Files);
        assert!(!matches!(app.center_mode, CenterMode::Diff { .. }));

        // Simulate the render cycle that happens between clicks: the pane is
        // now focused so the hint bar appears, shrinking the list area by 2
        // rows at the bottom (height 18 → 16).
        app.mouse_layout.unstaged_list = Some(Rect::new(78, 1, 21, 16));

        // Second click at the same position must still detect the double-click.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 1));
        assert_eq!(app.focus, FocusPane::Center);
        assert!(matches!(app.center_mode, CenterMode::Diff { .. }));
    }

    #[test]
    fn mouse_wheel_center_diff_scrolls_lines() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.center_mode = CenterMode::Diff {
            lines: Arc::new(vec![
                Line::from("one"),
                Line::from("two"),
                Line::from("three"),
            ]),
            scroll: 0,
            worktree_path: String::new(),
            rel_path: String::new(),
        };
        app.last_diff_height = 2;
        app.last_diff_visual_lines = 10;

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 30, 5));

        match app.center_mode {
            CenterMode::Diff { scroll, .. } => assert_eq!(scroll, 3),
            _ => panic!("expected diff mode"),
        }
    }

    #[test]
    fn mouse_drag_left_divider_updates_widths() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 19, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 5));

        assert!(app.left_width_pct > 20);
        assert!(app.left_width_pct + app.right_width_pct <= 80);
    }

    #[test]
    fn mouse_click_commit_text_first_click_only_focuses_commit_input() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.commit_input = TextInput::with_text("hello world".to_string());
        app.focus = FocusPane::Center;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 15));

        assert_eq!(app.focus, FocusPane::Files);
        assert_eq!(app.right_section, RightSection::CommitInput);
        assert_eq!(app.input_target, InputTarget::None);
    }

    #[test]
    fn mouse_click_commit_text_second_click_enters_edit_mode_and_moves_cursor() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.commit_input = TextInput::with_text("abc\ndef".to_string());
        app.commit_input.cursor = 0;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 16));
        assert_eq!(app.input_target, InputTarget::None);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 16));

        assert_eq!(app.input_target, InputTarget::CommitMessage);
        assert!(app.commit_input.cursor > 0);
    }

    #[test]
    fn mouse_journey_commit_chrome_then_text_enters_editing() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.commit_input = TextInput::with_text("hello".to_string());
        app.focus = FocusPane::Center;

        // Click inside the commit block (below the border row that the divider
        // occupies) so that the first click focuses the commit section.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 15));
        assert_eq!(app.focus, FocusPane::Files);
        assert_eq!(app.right_section, RightSection::CommitInput);
        assert_eq!(app.input_target, InputTarget::None);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 16));
        assert_eq!(app.input_target, InputTarget::CommitMessage);
    }

    #[test]
    fn mouse_wheel_commit_text_scrolls_commit_viewport() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.commit_input =
            TextInput::with_text((0..20).map(|i| format!("line {i}\n")).collect::<String>())
                .with_multiline(4);

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 80, 15));

        assert!(app.commit_input.scroll_offset() > 0);
        assert_eq!(app.right_section, RightSection::CommitInput);
    }

    #[test]
    fn mouse_journey_divider_drag_persists_widths_on_release() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        let original_left = app.config.ui.left_width_pct;
        let original_right = app.config.ui.right_width_pct;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 19, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 5));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 30, 5));

        assert_ne!(app.left_width_pct, original_left);
        assert_eq!(app.config.ui.left_width_pct, app.left_width_pct);
        assert_eq!(app.config.ui.right_width_pct, app.right_width_pct);
        assert_eq!(app.config.ui.right_width_pct, original_right);
    }

    #[test]
    fn mouse_drag_staged_divider_updates_height() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        // Set up a layout with a gap between unstaged and staged content areas
        // to represent the border row where the divider lives.
        // unstaged inner content: rows 1..7 (y=1, height=6)
        // border gap: row 7
        // staged inner content: rows 8..12 (y=8, height=5)
        app.mouse_layout.unstaged_list = Some(Rect::new(78, 1, 21, 6));
        app.mouse_layout.staged_list = Some(Rect::new(78, 8, 21, 5));
        app.unstaged_files = vec![ChangedFile {
            path: "a.txt".into(),
            status: "M".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        }];
        app.staged_files = vec![ChangedFile {
            path: "b.txt".into(),
            status: "A".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        }];
        let original = app.staged_pane_height_pct;

        // Click on the gap row (7), which is inside the divider zone.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 7));
        assert!(app.mouse_drag.is_some());

        // Drag downward to shrink the staged section.
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 80, 12));
        assert_ne!(app.staged_pane_height_pct, original);

        // Release persists.
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 80, 12));
        assert_eq!(
            app.config.ui.staged_pane_height_pct,
            app.staged_pane_height_pct
        );
    }

    #[test]
    fn mouse_staged_divider_not_detected_without_staged_files() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        // With no staged files, staged_list is None — divider must not appear.
        app.mouse_layout.staged_list = None;
        app.mouse_layout.unstaged_list = Some(Rect::new(78, 1, 21, 18));

        // Click on a row that would be the divider if staged_list existed.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 9));
        assert!(app.mouse_drag.is_none());
    }

    #[test]
    fn mouse_drag_staged_divider_upward_grows_staged_section() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.mouse_layout.unstaged_list = Some(Rect::new(78, 1, 21, 6));
        app.mouse_layout.staged_list = Some(Rect::new(78, 8, 21, 5));
        app.unstaged_files = vec![ChangedFile {
            path: "a.txt".into(),
            status: "M".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        }];
        app.staged_files = vec![ChangedFile {
            path: "b.txt".into(),
            status: "A".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        }];
        let original = app.staged_pane_height_pct;

        // Click divider gap row.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 7));
        assert!(app.mouse_drag.is_some());

        // Drag upward to grow the staged section.
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 80, 3));
        assert!(app.staged_pane_height_pct > original);
    }

    #[test]
    fn mouse_drag_commit_divider_updates_height() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        // staged inner content: rows 9..12 (y=9, height=3)
        // border gap: row 12
        // commit_area: rows 13..19 (y=13, height=6) — includes border
        app.mouse_layout.staged_list = Some(Rect::new(78, 9, 21, 3));
        app.mouse_layout.commit_area = Some(Rect::new(77, 13, 23, 6));
        app.staged_files = vec![ChangedFile {
            path: "b.txt".into(),
            status: "A".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        }];
        let original = app.commit_pane_height_pct;

        // Click on the gap row (12), which is inside the divider zone.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 12));
        assert!(app.mouse_drag.is_some());

        // Drag upward to grow the commit section.
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 80, 10));
        assert!(app.commit_pane_height_pct > original);

        // Release persists.
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 80, 10));
        assert_eq!(
            app.config.ui.commit_pane_height_pct,
            app.commit_pane_height_pct
        );
    }

    #[test]
    fn mouse_commit_divider_not_detected_without_staged_files() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        // No staged files means no staged_list and no commit_area.
        app.mouse_layout.staged_list = None;
        app.mouse_layout.commit_area = None;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 12));
        assert!(app.mouse_drag.is_none());
    }

    #[test]
    fn mouse_drag_commit_divider_downward_shrinks_commit_section() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.mouse_layout.staged_list = Some(Rect::new(78, 9, 21, 3));
        app.mouse_layout.commit_area = Some(Rect::new(77, 13, 23, 6));
        app.staged_files = vec![ChangedFile {
            path: "b.txt".into(),
            status: "A".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        }];
        let original = app.commit_pane_height_pct;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 12));
        assert!(app.mouse_drag.is_some());

        // Drag downward to shrink the commit section.
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 80, 16));
        assert!(app.commit_pane_height_pct < original);
    }

    #[test]
    fn command_palette_jk_keys_insert_text_instead_of_navigating() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::Command {
            input: TextInput::new(),
            selected: 0,
        };

        // Press 'j' — should insert into text, not move selection down.
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
            .unwrap();
        match &app.prompt {
            PromptState::Command {
                input, selected, ..
            } => {
                assert_eq!(input.text, "j");
                assert_eq!(*selected, 0, "selection should stay at 0, not move down");
            }
            other => panic!("expected command prompt, got {other:?}"),
        }

        // Press 'k' — should also insert, not move selection up.
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
            .unwrap();
        match &app.prompt {
            PromptState::Command { input, .. } => {
                assert_eq!(input.text, "jk");
            }
            other => panic!("expected command prompt, got {other:?}"),
        }
    }

    #[test]
    fn mouse_click_command_palette_row_selects_then_double_click_executes() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::Command {
            input: TextInput::with_text("help".to_string()),
            selected: 0,
        };
        install_command_overlay(&mut app, 1);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 15, 9));
        assert!(matches!(
            app.prompt,
            PromptState::Command { selected: 0, .. }
        ));

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 15, 9));
        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(app.help_scroll, Some(0));
    }

    #[test]
    fn mouse_click_command_palette_input_moves_cursor_and_keyboard_inserts_there() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::Command {
            input: TextInput::with_text("help".to_string()),
            selected: 0,
        };
        install_command_overlay(&mut app, 1);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 19, 7));
        app.handle_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::Command { input, .. } => {
                assert_eq!(input.text, "heXlp");
                assert_eq!(input.cursor, 3);
            }
            other => panic!("expected command prompt, got {other:?}"),
        }
    }

    #[test]
    fn mouse_click_project_browser_row_selects_then_double_click_opens_entry() {
        let mut app = test_app(default_bindings());
        let root = PathBuf::from(&app.projects[0].path);
        let child = root.join("child");
        std::fs::create_dir_all(&child).expect("child dir");
        app.prompt = PromptState::BrowseProjects {
            current_dir: root.clone(),
            entries: vec![crate::app::BrowserEntry {
                path: child.clone(),
                label: "child/".to_string(),
                is_git_repo: false,
            }],
            loading: false,
            selected: 0,
            filter: TextInput::new(),
            searching: false,
            editing_path: false,
            path_input: TextInput::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        };
        install_browser_overlay(&mut app, 1);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 15, 6));
        assert!(matches!(
            app.prompt,
            PromptState::BrowseProjects { selected: 0, .. }
        ));

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 15, 6));
        match &app.prompt {
            PromptState::BrowseProjects {
                current_dir,
                loading,
                selected,
                ..
            } => {
                assert_eq!(current_dir, &child);
                assert!(*loading);
                assert_eq!(*selected, 0);
            }
            _ => panic!("expected browse projects prompt"),
        }
    }

    #[test]
    fn mouse_click_browser_search_input_moves_cursor_and_edits_filter() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::BrowseProjects {
            current_dir: PathBuf::from(&app.projects[0].path),
            entries: Vec::new(),
            loading: false,
            selected: 0,
            filter: TextInput::with_text("child".to_string()),
            searching: false,
            editing_path: false,
            path_input: TextInput::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        };
        install_browser_overlay(&mut app, 0);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 19, 4));
        app.handle_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::BrowseProjects {
                filter, searching, ..
            } => {
                assert_eq!(filter.text, "chXild");
                assert_eq!(filter.cursor, 3);
                assert!(*searching);
            }
            other => panic!("expected browse projects prompt, got {other:?}"),
        }
    }

    #[test]
    fn mouse_click_browser_path_input_moves_cursor_and_edits_path() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::BrowseProjects {
            current_dir: PathBuf::from(&app.projects[0].path),
            entries: Vec::new(),
            loading: false,
            selected: 0,
            filter: TextInput::new(),
            searching: false,
            editing_path: true,
            path_input: TextInput::with_text("abcd".to_string()),
            tab_completions: Vec::new(),
            tab_index: 0,
        };
        install_browser_overlay(&mut app, 0);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 20, 4));
        app.handle_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::BrowseProjects { path_input, .. } => {
                assert_eq!(path_input.text, "aXbcd");
                assert_eq!(path_input.cursor, 2);
            }
            other => panic!("expected browse projects prompt, got {other:?}"),
        }
    }

    #[test]
    fn mouse_click_pick_editor_row_selects_then_double_click_opens_editor() {
        let mut app = test_app(default_bindings());
        std::fs::create_dir_all(&app.sessions[0].worktree_path).expect("worktree");
        app.prompt = PromptState::PickEditor {
            session_label: "agent-branch".to_string(),
            worktree_path: app.sessions[0].worktree_path.clone(),
            editors: vec![DetectedEditor {
                kind: EditorKind::VsCode,
                label: "VS Code",
                config_key: "vscode",
                command: "true".to_string(),
            }],
            selected: 0,
        };
        install_pick_editor_overlay(&mut app, 1);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 20, 8));
        assert!(matches!(
            app.prompt,
            PromptState::PickEditor { selected: 0, .. }
        ));

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 20, 8));
        assert!(matches!(app.prompt, PromptState::None));
        assert!(app.status.text().contains("Opened agent"));
    }

    #[test]
    fn mouse_click_kill_running_row_toggles_selection() {
        let mut app = test_app(default_bindings());
        let runtime_id = RuntimeTargetId::Agent("session-1".to_string());
        app.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes: vec![sample_runtime(
                runtime_id.clone(),
                KillableRuntimeKind::Agent,
                "agent-branch",
                "demo / codex / agent-branch",
            )],
            filter: TextInput::new(),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: std::collections::HashSet::new(),
            focus: KillRunningFocus::List,
        });
        install_kill_running_overlay(&mut app, 1);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 12, 6));

        match &app.prompt {
            PromptState::KillRunning(prompt) => {
                assert!(prompt.selected_ids.contains(&runtime_id));
            }
            other => panic!("expected kill-running prompt, got {other:?}"),
        }
    }

    #[test]
    fn mouse_click_kill_selected_button_opens_confirmation() {
        let mut app = test_app(default_bindings());
        let runtime_id = RuntimeTargetId::Agent("session-1".to_string());
        app.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes: vec![sample_runtime(
                runtime_id.clone(),
                KillableRuntimeKind::Agent,
                "agent-branch",
                "demo / codex / agent-branch",
            )],
            filter: TextInput::new(),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: std::iter::once(runtime_id).collect(),
            focus: KillRunningFocus::Footer(KillRunningFooterAction::Selected),
        });
        install_kill_running_overlay(&mut app, 1);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 47, 16));

        match &app.prompt {
            PromptState::ConfirmKillRunning(confirm_prompt) => {
                assert_eq!(confirm_prompt.action, KillRunningAction::Selected);
                assert_eq!(confirm_prompt.target_ids.len(), 1);
            }
            other => panic!("expected confirm kill prompt, got {other:?}"),
        }
    }

    #[test]
    fn mouse_click_quit_dialog_buttons_cancel_or_exit() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ConfirmQuit {
            agent_count: 1,
            terminal_count: 0,
            confirm_selected: false,
        };
        install_confirm_quit_overlay(&mut app);
        let should_quit = app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 35, 10));
        assert!(!should_quit);
        assert!(matches!(app.prompt, PromptState::None));

        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ConfirmQuit {
            agent_count: 1,
            terminal_count: 0,
            confirm_selected: true,
        };
        install_confirm_quit_overlay(&mut app);
        let should_quit = app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 53, 10));
        assert!(should_quit);
        assert!(matches!(app.prompt, PromptState::None));
    }

    #[test]
    fn quit_prompt_counts_running_companion_terminals_and_agents() {
        let mut app = test_app(default_bindings());
        let worktree = std::path::Path::new(&app.sessions[0].worktree_path);
        let (command, args) = ("/bin/sh", vec!["-c".to_string(), "sleep 5".to_string()]);
        let provider =
            PtyClient::spawn(command, &args, worktree, 24, 80, 1_000).expect("spawn test agent");
        let session_id = app.sessions[0].id.clone();
        app.providers.insert(session_id.clone(), provider);
        // Insert a companion terminal to simulate a running terminal.
        let term_client =
            PtyClient::spawn(command, &args, worktree, 24, 80, 1_000).expect("spawn test terminal");
        app.companion_terminals.insert(
            "term-test".to_string(),
            crate::app::CompanionTerminal {
                session_id,
                label: "test".to_string(),
                foreground_cmd: None,
                client: term_client,
            },
        );

        let should_quit = app
            .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .expect("handle quit");

        assert!(!should_quit);
        match app.prompt {
            PromptState::ConfirmQuit {
                agent_count,
                terminal_count,
                ..
            } => {
                assert_eq!(agent_count, 1);
                assert_eq!(terminal_count, 1);
            }
            _ => panic!("expected quit confirmation"),
        }
    }

    #[test]
    fn cycle_provider_only_updates_new_and_stopped_agents() {
        let mut app = test_app(default_bindings());
        let root = PathBuf::from(&app.projects[0].path);
        let now = Utc::now();

        app.config.projects.push(ProjectConfig {
            id: app.projects[0].id.clone(),
            path: app.projects[0].path.clone(),
            name: Some(app.projects[0].name.clone()),
            default_provider: Some(app.projects[0].default_provider.as_str().to_string()),
            commit_prompt: None,
        });

        app.sessions[0].provider = ProviderKind::from_str("codex");
        app.sessions[0].status = SessionStatus::Active;
        app.session_store
            .upsert_session(&app.sessions[0])
            .expect("persist active session");

        let detached = AgentSession {
            id: "session-2".to_string(),
            project_id: app.projects[0].id.clone(),
            project_path: Some(app.projects[0].path.clone()),
            provider: ProviderKind::from_str("codex"),
            source_branch: "main".to_string(),
            branch_name: "detached-branch".to_string(),
            worktree_path: app
                .paths
                .worktrees_root
                .join("detached")
                .display()
                .to_string(),
            title: None,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
        };
        let exited = AgentSession {
            id: "session-3".to_string(),
            project_id: app.projects[0].id.clone(),
            project_path: Some(app.projects[0].path.clone()),
            provider: ProviderKind::from_str("codex"),
            source_branch: "main".to_string(),
            branch_name: "exited-branch".to_string(),
            worktree_path: app
                .paths
                .worktrees_root
                .join("exited")
                .display()
                .to_string(),
            title: None,
            status: SessionStatus::Exited,
            created_at: now,
            updated_at: now,
        };
        let other_project = Project {
            id: "project-2".to_string(),
            name: "other".to_string(),
            path: root.join("other-project").display().to_string(),
            default_provider: ProviderKind::from_str("codex"),
            current_branch: "main".to_string(),
        };
        let other_session = AgentSession {
            id: "session-4".to_string(),
            project_id: other_project.id.clone(),
            project_path: Some(other_project.path.clone()),
            provider: ProviderKind::from_str("codex"),
            source_branch: "main".to_string(),
            branch_name: "other-branch".to_string(),
            worktree_path: app.paths.worktrees_root.join("other").display().to_string(),
            title: None,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
        };

        app.config.projects.push(ProjectConfig {
            id: other_project.id.clone(),
            path: other_project.path.clone(),
            name: Some(other_project.name.clone()),
            default_provider: Some(other_project.default_provider.as_str().to_string()),
            commit_prompt: None,
        });
        app.projects.push(other_project);
        app.sessions.push(detached);
        app.sessions.push(exited);
        app.sessions.push(other_session);
        for session in &app.sessions[1..] {
            app.session_store
                .upsert_session(session)
                .expect("persist session");
        }

        app.rebuild_left_items();
        app.selected_left = 0;

        app.cycle_selected_project_provider()
            .expect("cycle provider");

        // With providers [claude, codex, gemini, opencode], cycling from codex
        // advances to the next provider: gemini.
        assert_eq!(app.projects[0].default_provider.as_str(), "gemini");
        assert_eq!(
            app.config.projects[0].default_provider.as_deref(),
            Some("gemini")
        );
        // Active session keeps its original provider.
        assert_eq!(app.sessions[0].provider.as_str(), "codex");
        // Non-active sessions are updated to the new default.
        assert_eq!(app.sessions[1].provider.as_str(), "gemini");
        assert_eq!(app.sessions[2].provider.as_str(), "gemini");
        // Session belonging to a different project is untouched.
        assert_eq!(app.sessions[3].provider.as_str(), "codex");

        let persisted = app.session_store.load_sessions().expect("load sessions");
        let provider_for = |id: &str| {
            persisted
                .iter()
                .find(|session| session.id == id)
                .map(|session| session.provider.as_str())
        };
        assert_eq!(provider_for("session-1"), Some("codex"));
        assert_eq!(provider_for("session-2"), Some("gemini"));
        assert_eq!(provider_for("session-3"), Some("gemini"));
        assert_eq!(provider_for("session-4"), Some("codex"));

        assert!(
            app.status
                .text()
                .contains("Changed default CLI agent to \"gemini\"")
        );
    }

    #[test]
    fn launch_companion_terminal_sets_runtime_state_and_overlay() {
        let mut app = test_app(default_bindings());

        app.show_companion_terminal()
            .expect("launch companion terminal");

        assert_eq!(
            app.selected_companion_terminal_status(),
            CompanionTerminalStatus::Running
        );
        assert_eq!(app.companion_terminals.len(), 1);
        assert!(app.active_terminal_id.is_some());
        assert_eq!(app.session_surface, SessionSurface::Terminal);
        assert_eq!(app.input_target, InputTarget::Terminal);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);
    }

    #[test]
    fn multiple_terminals_per_session() {
        let mut app = test_app(default_bindings());

        app.show_companion_terminal().expect("first terminal");
        let first_id = app.active_terminal_id.clone().unwrap();

        // Close overlay, launch another.
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.input_target = InputTarget::None;
        app.session_surface = SessionSurface::Agent;

        app.show_companion_terminal().expect("second terminal");
        let second_id = app.active_terminal_id.clone().unwrap();

        assert_ne!(first_id, second_id);
        assert_eq!(app.companion_terminals.len(), 2);
        assert_eq!(app.terminal_items().len(), 2);
    }

    #[test]
    fn exit_interactive_from_terminal_overlay_resets_state() {
        let mut app = test_app(default_bindings());

        app.show_companion_terminal()
            .expect("launch companion terminal");

        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);
        assert_eq!(app.input_target, InputTarget::Terminal);

        // Simulate ExitInteractive via the raw input path: feed Ctrl-G
        // (0x07) into the raw input buffer and process sequences.
        app.raw_input_buf = vec![0x07];
        let (sequences, _) = crate::raw_input::split_sequences(&app.raw_input_buf);
        assert_eq!(sequences.len(), 1);
        let matched = app.interactive_patterns.match_sequence(sequences[0]);
        assert_eq!(matched, Some((Action::ExitInteractive, false)));

        // Apply the same state change that poll_and_forward_raw_input does.
        let return_to_list =
            matches!(app.input_target, InputTarget::Terminal) && app.terminal_return_to_list;
        app.input_target = InputTarget::None;
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.session_surface = SessionSurface::Agent;
        app.raw_input_buf.clear();
        if return_to_list {
            app.left_section = LeftSection::Terminals;
            app.focus = FocusPane::Left;
        }

        // Launched via `t` → returns to terminals list on the left pane.
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
        assert_eq!(app.session_surface, SessionSurface::Agent);
        assert_eq!(app.input_target, InputTarget::None);
        assert_eq!(app.left_section, LeftSection::Terminals);
        assert_eq!(app.focus, FocusPane::Left);
    }

    #[test]
    fn ctrl_g_enters_interactive_mode_from_center_pane() {
        let mut app = test_app(default_bindings());
        app.focus = FocusPane::Center;
        app.center_mode = CenterMode::Agent;
        app.providers.insert(
            "session-1".to_string(),
            PtyClient::spawn(
                "sh",
                &["-c".to_string(), "printf ready; sleep 0.2".to_string()],
                std::path::Path::new("."),
                10,
                10,
                100,
            )
            .expect("spawn pty"),
        );

        // Ctrl+G from non-interactive center pane should enter interactive mode.
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL))
            .unwrap();

        assert_eq!(app.input_target, InputTarget::Agent);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Agent);
    }

    #[test]
    fn close_top_overlay_closes_terminal_overlay() {
        let mut app = test_app(default_bindings());

        app.show_companion_terminal()
            .expect("launch companion terminal");

        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);

        let closed = app.close_top_overlay();
        assert!(closed);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
        assert_eq!(app.session_surface, SessionSurface::Agent);
        // Terminal PTY should still be alive in the map.
        assert_eq!(app.companion_terminals.len(), 1);
    }

    #[test]
    fn session_switch_closes_terminal_overlay_but_keeps_pty() {
        let mut app = test_app(default_bindings());

        app.show_companion_terminal()
            .expect("launch companion terminal");

        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);

        // Switch to project row (index 0) — simulates user clicking a different item.
        app.set_left_selection(0);

        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
        // Terminal PTY should still be alive.
        assert_eq!(app.companion_terminals.len(), 1);
    }

    #[test]
    fn terminal_items_returns_running_terminals() {
        let mut app = test_app(default_bindings());

        // No terminals launched yet.
        assert!(app.terminal_items().is_empty());

        // Launch a terminal.
        app.show_companion_terminal()
            .expect("launch companion terminal");

        assert_eq!(app.terminal_items().len(), 1);
    }

    #[test]
    fn left_section_navigation_crosses_to_terminals() {
        let mut app = test_app(default_bindings());
        app.show_companion_terminal()
            .expect("launch companion terminal");
        // Close overlay and go back to projects section.
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.input_target = InputTarget::None;
        app.session_surface = SessionSurface::Agent;
        app.left_section = LeftSection::Projects;
        app.focus = FocusPane::Left;

        // Navigate down past the last project item.
        let item_count = app.left_items().len();
        app.selected_left = item_count - 1;

        let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        app.handle_key(down).unwrap();

        assert_eq!(app.left_section, LeftSection::Terminals);
        assert_eq!(app.selected_terminal_index, 0);
    }

    #[test]
    fn header_running_terminal_count() {
        let mut app = test_app(default_bindings());

        assert_eq!(app.running_companion_terminal_count(), 0);

        let session_id = app.sessions[0].id.clone();
        let worktree = std::path::Path::new(&app.sessions[0].worktree_path);
        let (command, args) = ("/bin/sh", vec!["-c".to_string(), "sleep 2".to_string()]);
        let client = PtyClient::spawn(command, &args, worktree, 24, 80, 1_000).expect("spawn");
        app.companion_terminals.insert(
            "term-1".to_string(),
            crate::app::CompanionTerminal {
                session_id,
                label: "test".to_string(),
                foreground_cmd: None,
                client,
            },
        );

        assert_eq!(app.running_companion_terminal_count(), 1);
    }

    #[test]
    fn show_or_open_first_terminal_spawns_when_none_exist() {
        let mut app = test_app(default_bindings());

        assert!(app.companion_terminals.is_empty());

        app.show_or_open_first_terminal()
            .expect("should spawn new terminal");

        assert_eq!(app.companion_terminals.len(), 1);
        assert!(app.active_terminal_id.is_some());
        assert_eq!(app.session_surface, SessionSurface::Terminal);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);
        assert_eq!(app.input_target, InputTarget::Terminal);
        // Spawned via fallback — returns to terminal list on close.
        assert!(app.terminal_return_to_list);
    }

    #[test]
    fn show_or_open_first_terminal_reuses_existing() {
        let mut app = test_app(default_bindings());

        // Spawn an initial terminal.
        app.show_companion_terminal().expect("spawn first");
        let first_id = app.active_terminal_id.clone().unwrap();
        assert_eq!(app.companion_terminals.len(), 1);

        // Close overlay to simulate returning to normal view.
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.input_target = InputTarget::None;
        app.session_surface = SessionSurface::Agent;

        // Now use the "open first" method — it should reuse, not spawn.
        app.show_or_open_first_terminal()
            .expect("should reuse existing terminal");

        assert_eq!(app.companion_terminals.len(), 1, "no new terminal spawned");
        assert_eq!(app.active_terminal_id.as_deref(), Some(first_id.as_str()));
        assert_eq!(app.session_surface, SessionSurface::Terminal);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);
        assert_eq!(app.input_target, InputTarget::Terminal);
        // Reused — should NOT return to terminal list on close.
        assert!(!app.terminal_return_to_list);
    }

    #[test]
    fn show_or_open_first_terminal_picks_lowest_id() {
        let mut app = test_app(default_bindings());

        // Spawn two terminals.
        app.show_companion_terminal().expect("first");
        let first_id = app.active_terminal_id.clone().unwrap();
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.input_target = InputTarget::None;
        app.session_surface = SessionSurface::Agent;

        app.show_companion_terminal().expect("second");
        let second_id = app.active_terminal_id.clone().unwrap();
        assert_ne!(first_id, second_id);
        assert_eq!(app.companion_terminals.len(), 2);

        // Close overlay.
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.input_target = InputTarget::None;
        app.session_surface = SessionSurface::Agent;

        // Should pick the first (lowest ID), not the second.
        app.show_or_open_first_terminal()
            .expect("should reuse first");

        assert_eq!(app.active_terminal_id.as_deref(), Some(first_id.as_str()));
        assert_eq!(app.companion_terminals.len(), 2, "still two terminals");
    }

    #[test]
    fn spawn_terminal_for_selected_terminal_creates_new() {
        let mut app = test_app(default_bindings());

        // Spawn an initial terminal so the terminals list is populated.
        app.show_companion_terminal().expect("initial terminal");
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.input_target = InputTarget::None;
        app.session_surface = SessionSurface::Agent;

        // Point the terminal cursor at the first terminal.
        app.left_section = LeftSection::Terminals;
        app.selected_terminal_index = 0;

        let initial_count = app.companion_terminals.len();
        app.spawn_terminal_for_selected_terminal()
            .expect("spawn from terminals pane");

        assert_eq!(
            app.companion_terminals.len(),
            initial_count + 1,
            "a new terminal was created"
        );
        assert_eq!(app.session_surface, SessionSurface::Terminal);
        assert_eq!(app.input_target, InputTarget::Terminal);
        assert!(app.terminal_return_to_list);
    }

    #[test]
    fn new_companion_terminal_warns_without_selected_session() {
        let mut app = test_app(default_bindings());

        // Deselect everything by pointing at a project header.
        app.selected_left = 0;

        app.new_companion_terminal()
            .expect("should not error, just warn");

        assert_eq!(
            app.status.tone(),
            crate::statusline::StatusTone::Warning,
            "should show yellow warning, not red error"
        );
        assert!(app.status.text().contains("Select an agent session"));
        assert!(
            app.companion_terminals.is_empty(),
            "no terminal should be spawned"
        );
    }

    #[test]
    fn new_companion_terminal_spawns_when_session_selected() {
        let mut app = test_app(default_bindings());

        app.new_companion_terminal()
            .expect("should spawn new terminal");

        assert_eq!(app.companion_terminals.len(), 1);
        assert!(app.active_terminal_id.is_some());
        assert_eq!(app.input_target, InputTarget::Terminal);
    }

    #[test]
    fn mouse_click_discard_dialog_button_discards_changes() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        init_git_repo_with_modified_file(
            &app,
            "src/main.rs",
            "fn main() {}\n",
            "fn main() { println!(\"hi\"); }\n",
        );
        app.selected_left = 1;
        app.unstaged_files = vec![ChangedFile {
            path: "src/main.rs".into(),
            status: "M".into(),
            additions: 1,
            deletions: 1,
            binary: false,
        }];
        app.prompt = PromptState::ConfirmDiscardFile {
            file_path: "src/main.rs".to_string(),
            is_untracked: false,
            confirm_selected: true,
        };
        install_confirm_discard_overlay(&mut app);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 53, 10));

        assert!(matches!(app.prompt, PromptState::None));
        let contents = std::fs::read_to_string(
            PathBuf::from(&app.sessions[0].worktree_path).join("src/main.rs"),
        )
        .expect("discarded file");
        assert_eq!(contents, "fn main() {}\n");
    }

    #[test]
    fn mouse_click_delete_dialog_cancel_button_closes_prompt() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ConfirmDeleteAgent {
            session_id: app.sessions[0].id.clone(),
            branch_name: app.sessions[0].branch_name.clone(),
            confirm_selected: false,
        };
        install_confirm_delete_overlay(&mut app);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 35, 10));

        assert!(matches!(app.prompt, PromptState::None));
    }

    #[test]
    fn mouse_click_rename_input_moves_cursor() {
        let mut app = test_app(default_bindings());
        let sid = app.sessions[0].id.clone();
        app.prompt = PromptState::RenameSession {
            session_id: sid,
            input: TextInput::with_text("rename me".to_string()),
            rename_branch: false,
        };
        install_rename_overlay(&mut app);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 29, 10));

        match &app.prompt {
            PromptState::RenameSession { input, .. } => assert_eq!(input.cursor, 5),
            _ => panic!("expected rename prompt"),
        }
    }

    #[test]
    fn mouse_click_while_prompt_open_does_not_change_underlying_focus() {
        let mut app = test_app(default_bindings());
        app.focus = FocusPane::Left;
        app.prompt = PromptState::Command {
            input: TextInput::with_text("help".to_string()),
            selected: 0,
        };
        install_command_overlay(&mut app, 1);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));

        assert_eq!(app.focus, FocusPane::Left);
        assert!(matches!(app.prompt, PromptState::Command { .. }));
    }

    #[test]
    fn toggle_git_pane_collapses_right() {
        let mut app = test_app(default_bindings());
        assert!(!app.right_collapsed);

        app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE))
            .unwrap();
        assert!(app.right_collapsed);

        app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE))
            .unwrap();
        assert!(!app.right_collapsed);
    }

    #[test]
    fn collapse_git_pane_moves_focus_from_files() {
        let mut app = test_app(default_bindings());
        app.focus = FocusPane::Files;

        app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE))
            .unwrap();

        assert!(app.right_collapsed);
        assert_eq!(app.focus, FocusPane::Center);
    }

    #[test]
    fn remove_git_pane_hides_right() {
        let mut app = test_app(default_bindings());
        assert!(!app.right_hidden);

        app.execute_command("toggle-remove-git-pane".to_string())
            .unwrap();
        assert!(app.right_hidden);

        app.execute_command("toggle-remove-git-pane".to_string())
            .unwrap();
        assert!(!app.right_hidden);
    }

    #[test]
    fn remove_git_pane_moves_focus_from_files() {
        let mut app = test_app(default_bindings());
        app.focus = FocusPane::Files;

        app.execute_command("toggle-remove-git-pane".to_string())
            .unwrap();

        assert!(app.right_hidden);
        assert_eq!(app.focus, FocusPane::Center);
    }

    #[test]
    fn focus_skips_removed_git_pane_forward() {
        let mut app = test_app(default_bindings());
        app.right_hidden = true;
        app.focus = FocusPane::Center;

        // Tab from Center should skip Files and go to Left
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();

        assert_eq!(app.focus, FocusPane::Left);
    }

    #[test]
    fn focus_skips_removed_git_pane_backward() {
        let mut app = test_app(default_bindings());
        app.right_hidden = true;
        app.focus = FocusPane::Left;

        // Shift-Tab from Left should skip Files and go to Center
        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT))
            .unwrap();

        assert_eq!(app.focus, FocusPane::Center);
    }

    #[test]
    fn resume_fallback_retries_with_fresh_session_on_quick_exit() {
        let mut app = test_app(default_bindings());
        let session_id = app.sessions[0].id.clone();
        let worktree = std::path::Path::new(&app.sessions[0].worktree_path);
        // Spawn a process that exits immediately without producing output.
        let args = vec!["-c".to_string(), "exit 1".to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000).expect("spawn quick-exit");
        app.providers.insert(session_id.clone(), client);
        app.mark_session_status(&session_id, SessionStatus::Active);
        app.resume_fallback_candidates.insert(session_id.clone());
        app.selected_left = 1;
        app.session_surface = SessionSurface::Agent;

        // Wait for the process to exit.
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.drain_events();

        // The fallback should have spawned a fresh session, so the provider
        // is still present and the session is active (not detached).
        assert!(
            app.providers.contains_key(&session_id),
            "provider should still be present after fallback retry"
        );
        assert_eq!(app.sessions[0].status, SessionStatus::Active);
        assert!(
            !app.resume_fallback_candidates.contains(&session_id),
            "candidate should have been removed after fallback"
        );
        assert!(
            app.status.text().contains("No prior session to resume"),
            "status should inform user about fallback: {:?}",
            app.status.text()
        );
    }

    #[test]
    fn resume_fallback_skipped_when_pty_had_output() {
        let mut app = test_app(default_bindings());
        let session_id = app.sessions[0].id.clone();
        let worktree = std::path::Path::new(&app.sessions[0].worktree_path);
        // Spawn a process that produces output before exiting.
        let args = vec!["-c".to_string(), "echo hello".to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000).expect("spawn with output");
        app.providers.insert(session_id.clone(), client);
        app.mark_session_status(&session_id, SessionStatus::Active);
        app.resume_fallback_candidates.insert(session_id.clone());
        app.selected_left = 1;
        app.session_surface = SessionSurface::Agent;

        // Wait for the process to produce output and exit.
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.drain_events();

        // Since the PTY had output, the fallback should NOT have triggered.
        // The session should be detached (normal exit behavior).
        assert_eq!(app.sessions[0].status, SessionStatus::Detached);
        assert!(
            !app.resume_fallback_candidates.contains(&session_id),
            "candidate should have been removed even when skipped"
        );
    }

    #[test]
    fn no_fallback_for_non_candidate_sessions() {
        let mut app = test_app(default_bindings());
        let session_id = app.sessions[0].id.clone();
        let worktree = std::path::Path::new(&app.sessions[0].worktree_path);
        // Spawn a process that exits immediately, but do NOT add it as a candidate.
        let args = vec!["-c".to_string(), "exit 1".to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, worktree, 24, 80, 1_000).expect("spawn quick-exit");
        app.providers.insert(session_id.clone(), client);
        app.mark_session_status(&session_id, SessionStatus::Active);
        // Deliberately not adding to resume_fallback_candidates.
        app.selected_left = 1;
        app.session_surface = SessionSurface::Agent;

        std::thread::sleep(std::time::Duration::from_millis(200));
        app.drain_events();

        // Without being a candidate, the session should just go to detached.
        assert!(
            !app.providers.contains_key(&session_id),
            "provider should have been removed"
        );
        assert_eq!(app.sessions[0].status, SessionStatus::Detached);
    }

    // ── Scroll-mode input suppression tests ─────────────────────────

    /// Helper: set up an App with a live PTY in interactive mode and
    /// enough scrollback history so we can engage scroll mode.
    fn app_with_scrolled_back_pty() -> App {
        let mut app = test_app(default_bindings());
        let session_id = app.sessions[0].id.clone();

        // Spawn a shell that prints enough lines to fill the 5-row terminal
        // and produce scrollback history, then sleeps to stay alive.
        let args = vec![
            "-c".to_string(),
            "printf 'L1\\nL2\\nL3\\nL4\\nL5\\nL6\\nL7\\nL8\\nL9\\nL10\\n'; sleep 5".to_string(),
        ];
        let client = PtyClient::spawn("sh", &args, std::path::Path::new("."), 5, 40, 100)
            .expect("spawn pty");
        app.providers.insert(session_id, client);

        // Enter interactive agent mode.
        app.input_target = InputTarget::Agent;
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        app.last_pty_size = (5, 40);

        // Wait for the child to produce output so the PTY has content.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Scroll back so scrollback_offset > 0.
        let provider = app.selected_terminal_surface_client().unwrap();
        provider.set_scrollback(3);
        assert!(
            provider.scrollback_offset() > 0,
            "test setup: should be scrolled back"
        );

        app
    }

    #[test]
    fn scrolled_back_suppresses_regular_key_forwarding() {
        let mut app = app_with_scrolled_back_pty();
        // Feed a regular ASCII character 'x' (0x78) — should be dropped.
        let result = app.process_raw_input_bytes(b"x").unwrap();
        assert!(!result, "should not request exit");
        // The key was consumed without error; the PTY didn't receive it.
        // We can't directly assert write_bytes wasn't called without a mock,
        // but we verify the app didn't crash and the scrollback is unchanged.
        assert!(
            app.selected_terminal_surface_client()
                .unwrap()
                .scrollback_offset()
                > 0,
            "scrollback should remain engaged"
        );
    }

    #[test]
    fn scrolled_back_allows_exit_interactive() {
        let mut app = app_with_scrolled_back_pty();
        assert_eq!(app.input_target, InputTarget::Agent);

        // Feed Ctrl-G (0x07) — ExitInteractive should still work.
        let result = app.process_raw_input_bytes(&[0x07]).unwrap();
        assert!(!result);
        assert_eq!(
            app.input_target,
            InputTarget::None,
            "ExitInteractive must work even when scrolled back"
        );
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
    }

    #[test]
    fn scrolled_back_suppresses_macro_bar() {
        let mut app = app_with_scrolled_back_pty();

        // Add a macro so OpenMacroBar would normally open the bar.
        app.config.macros.entries.insert(
            "test".to_string(),
            crate::config::MacroEntry {
                text: "hello".to_string(),
                surface: crate::config::MacroSurface::Agent,
            },
        );

        // Find the byte pattern for OpenMacroBar (Ctrl-\, which is 0x1c).
        let matched = app.interactive_patterns.match_sequence(&[0x1c]);
        assert_eq!(
            matched.map(|(a, _)| a),
            Some(Action::OpenMacroBar),
            "Ctrl-\\ should resolve to OpenMacroBar"
        );

        // Feed Ctrl-\ while scrolled back.
        let result = app.process_raw_input_bytes(&[0x1c]).unwrap();
        assert!(!result);
        assert!(
            app.macro_bar.is_none(),
            "macro bar must not open when scrolled back"
        );
    }

    #[test]
    fn not_scrolled_back_forwards_normally() {
        let mut app = app_with_scrolled_back_pty();

        // Reset scrollback to 0 (live bottom).
        app.selected_terminal_surface_client()
            .unwrap()
            .set_scrollback(0);
        assert_eq!(
            app.selected_terminal_surface_client()
                .unwrap()
                .scrollback_offset(),
            0
        );

        // Feed a regular character — should be forwarded without error.
        let result = app.process_raw_input_bytes(b"x").unwrap();
        assert!(!result);
    }

    #[test]
    fn scrolled_back_allows_scroll_page_up() {
        let mut app = app_with_scrolled_back_pty();
        let before = app
            .selected_terminal_surface_client()
            .unwrap()
            .scrollback_offset();

        // Feed PgUp (CSI 5~) — should scroll further back.
        let result = app.process_raw_input_bytes(b"\x1b[5~").unwrap();
        assert!(!result);
        let after = app
            .selected_terminal_surface_client()
            .unwrap()
            .scrollback_offset();
        assert!(
            after >= before,
            "PgUp should increase or maintain scrollback offset"
        );
    }

    #[test]
    fn scrolled_back_allows_scroll_to_bottom() {
        let mut app = app_with_scrolled_back_pty();
        assert!(
            app.selected_terminal_surface_client()
                .unwrap()
                .scrollback_offset()
                > 0
        );

        // Feed 'q' (0x71) — ScrollToBottom should reset scrollback.
        let result = app.process_raw_input_bytes(b"q").unwrap();
        assert!(!result);
        assert_eq!(
            app.selected_terminal_surface_client()
                .unwrap()
                .scrollback_offset(),
            0,
            "'q' should scroll to bottom when scrolled back"
        );
    }

    // ── Click-outside-fullscreen tests ──────────────────────────────

    /// Build an SGR mouse left-button-down sequence at 1-based (cx, cy).
    fn sgr_mouse_down(cx: u16, cy: u16) -> Vec<u8> {
        format!("\x1b[<0;{cx};{cy}M").into_bytes()
    }

    /// Helper: set up an App with a live PTY in interactive agent mode
    /// and a mouse layout so we can test click-outside-overlay behavior.
    fn app_with_interactive_agent_pty() -> App {
        let mut app = test_app(default_bindings());
        let session_id = app.sessions[0].id.clone();

        let args = vec!["-c".to_string(), "sleep 5".to_string()];
        let client = PtyClient::spawn("sh", &args, std::path::Path::new("."), 5, 40, 100)
            .expect("spawn pty");
        app.providers.insert(session_id, client);

        app.input_target = InputTarget::Agent;
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        app.last_pty_size = (5, 40);
        install_mouse_layout(&mut app);
        app
    }

    #[test]
    fn click_outside_fullscreen_agent_exits_interactive_mode() {
        let mut app = app_with_interactive_agent_pty();
        assert_eq!(app.input_target, InputTarget::Agent);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Agent);

        // Click at (1,1) which is inside the left pane, outside agent_term
        // (agent_term starts at x=21). SGR coords are 1-based.
        let bytes = sgr_mouse_down(2, 2);
        let result = app.process_raw_input_bytes(&bytes).unwrap();
        assert!(!result);
        assert_eq!(
            app.input_target,
            InputTarget::None,
            "clicking outside overlay must exit interactive mode"
        );
        assert_eq!(
            app.fullscreen_overlay,
            FullscreenOverlay::None,
            "clicking outside overlay must dismiss fullscreen"
        );
    }

    #[test]
    fn click_inside_fullscreen_agent_stays_in_interactive_mode() {
        let mut app = app_with_interactive_agent_pty();
        assert_eq!(app.input_target, InputTarget::Agent);

        // Click at (30, 5) which is inside agent_term (x=21..76, y=1..17).
        // SGR coords are 1-based so (31, 6).
        let bytes = sgr_mouse_down(31, 6);
        let result = app.process_raw_input_bytes(&bytes).unwrap();
        assert!(!result);
        assert_eq!(
            app.input_target,
            InputTarget::Agent,
            "clicking inside overlay must stay in interactive mode"
        );
        assert_eq!(
            app.fullscreen_overlay,
            FullscreenOverlay::Agent,
            "clicking inside overlay must keep fullscreen"
        );
    }

    #[test]
    fn click_outside_fullscreen_terminal_exits_interactive_mode() {
        let mut app = app_with_interactive_agent_pty();
        // Switch to terminal interactive mode.
        app.input_target = InputTarget::Terminal;
        app.fullscreen_overlay = FullscreenOverlay::Terminal;
        app.session_surface = SessionSurface::Terminal;

        // Click outside agent_term.
        let bytes = sgr_mouse_down(2, 2);
        let result = app.process_raw_input_bytes(&bytes).unwrap();
        assert!(!result);
        assert_eq!(
            app.input_target,
            InputTarget::None,
            "clicking outside overlay must exit terminal interactive mode"
        );
        assert_eq!(
            app.fullscreen_overlay,
            FullscreenOverlay::None,
            "clicking outside overlay must dismiss terminal fullscreen"
        );
    }

    #[test]
    fn click_outside_fullscreen_terminal_returns_to_left_pane() {
        let mut app = app_with_interactive_agent_pty();
        app.input_target = InputTarget::Terminal;
        app.fullscreen_overlay = FullscreenOverlay::Terminal;
        app.session_surface = SessionSurface::Terminal;
        app.terminal_return_to_list = true;
        app.focus = FocusPane::Center;

        let bytes = sgr_mouse_down(2, 2);
        let result = app.process_raw_input_bytes(&bytes).unwrap();
        assert!(!result);
        assert_eq!(app.input_target, InputTarget::None);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
        assert_eq!(
            app.focus,
            FocusPane::Left,
            "focus should move to left pane when terminal_return_to_list is set"
        );
    }

    #[test]
    fn scrolled_back_allows_click_outside_exit() {
        // Use the scrolled-back helper (which prints enough lines to create
        // history) and install a mouse layout so click-outside detection works.
        let mut app = app_with_scrolled_back_pty();
        install_mouse_layout(&mut app);
        assert_eq!(app.input_target, InputTarget::Agent);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Agent);
        assert!(
            app.selected_terminal_surface_client()
                .unwrap()
                .scrollback_offset()
                > 0,
            "test setup: should be scrolled back"
        );

        // Click outside agent_term while scrolled back — overlay must close.
        let bytes = sgr_mouse_down(2, 2);
        let result = app.process_raw_input_bytes(&bytes).unwrap();
        assert!(!result);
        assert_eq!(
            app.input_target,
            InputTarget::None,
            "clicking outside overlay must exit interactive mode even when scrolled back"
        );
        assert_eq!(
            app.fullscreen_overlay,
            FullscreenOverlay::None,
            "clicking outside overlay must dismiss fullscreen even when scrolled back"
        );
    }

    // ---------------------------------------------------------------
    // Double-click detection: item-scoped
    // ---------------------------------------------------------------

    #[test]
    fn double_click_same_pane_same_item() {
        let bindings = default_bindings();
        let mut app = test_app(bindings);

        let first = app.register_mouse_click(MouseClickTarget::LeftPane, Some(2));
        assert!(!first, "first click must not be a double-click");

        let second = app.register_mouse_click(MouseClickTarget::LeftPane, Some(2));
        assert!(
            second,
            "second click on the same item within threshold must be a double-click"
        );
    }

    #[test]
    fn no_double_click_same_pane_different_item() {
        let bindings = default_bindings();
        let mut app = test_app(bindings);

        let first = app.register_mouse_click(MouseClickTarget::LeftPane, Some(2));
        assert!(!first);

        let second = app.register_mouse_click(MouseClickTarget::LeftPane, Some(5));
        assert!(
            !second,
            "clicking a different item in the same pane must NOT be a double-click"
        );
    }

    #[test]
    fn no_double_click_same_item_after_timeout() {
        let bindings = default_bindings();
        let mut app = test_app(bindings);

        let first = app.register_mouse_click(MouseClickTarget::LeftPane, Some(2));
        assert!(!first);

        // Simulate timeout by back-dating the stored click.
        if let Some(ref mut last) = app.last_mouse_click {
            last.at -= DOUBLE_CLICK_THRESHOLD + std::time::Duration::from_millis(1);
        }

        let second = app.register_mouse_click(MouseClickTarget::LeftPane, Some(2));
        assert!(
            !second,
            "clicking the same item after the threshold must NOT be a double-click"
        );
    }

    #[test]
    fn no_double_click_different_pane_same_index() {
        let bindings = default_bindings();
        let mut app = test_app(bindings);

        let first = app.register_mouse_click(MouseClickTarget::LeftPane, Some(0));
        assert!(!first);

        let second = app.register_mouse_click(MouseClickTarget::UnstagedPane, Some(0));
        assert!(
            !second,
            "clicking the same index in a different pane must NOT be a double-click"
        );
    }

    #[test]
    fn double_click_resets_after_trigger() {
        let bindings = default_bindings();
        let mut app = test_app(bindings);

        // First pair: triggers double-click.
        app.register_mouse_click(MouseClickTarget::LeftPane, Some(1));
        let triggered = app.register_mouse_click(MouseClickTarget::LeftPane, Some(1));
        assert!(triggered);

        // The state should be cleared — the next click starts a fresh sequence.
        let after = app.register_mouse_click(MouseClickTarget::LeftPane, Some(1));
        assert!(
            !after,
            "after a double-click triggers, the next click must start a new sequence"
        );
    }

    #[test]
    fn double_click_center_pane_no_item_index() {
        let bindings = default_bindings();
        let mut app = test_app(bindings);

        let first = app.register_mouse_click(MouseClickTarget::CenterPane, None);
        assert!(!first);

        let second = app.register_mouse_click(MouseClickTarget::CenterPane, None);
        assert!(
            second,
            "double-click on center pane (no item index) must work"
        );
    }
}
