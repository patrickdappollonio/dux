use super::render::{cursor_from_wrapped_position, wrap_text_at_width};
use super::*;
use alacritty_terminal::term::TermMode;

const MOUSE_WHEEL_LINES: usize = 3;
const MIN_LEFT_WIDTH_PCT: u16 = 14;
const MAX_LEFT_WIDTH_PCT: u16 = 38;
const MIN_RIGHT_WIDTH_PCT: u16 = 14;
const MAX_RIGHT_WIDTH_PCT: u16 = 50;
const MIN_CENTER_WIDTH_PCT: u16 = 20;
const DOUBLE_CLICK_THRESHOLD: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentWheelRoute {
    HostScrollback,
    ForwardMouse,
    ForwardAlternateScroll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseTarget {
    LeftPane,
    LeftRow(usize),
    Center,
    FilesPane,
    UnstagedFile(Option<usize>),
    StagedFile(Option<usize>),
    CommitChrome,
    CommitText,
}

fn contains_point(rect: Rect, column: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && column >= rect.x
        && column < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

fn clamp_left_width_pct(left_width_pct: u16, right_width_pct: u16) -> u16 {
    let max_left = MAX_LEFT_WIDTH_PCT.min(100 - MIN_CENTER_WIDTH_PCT - right_width_pct);
    left_width_pct.clamp(MIN_LEFT_WIDTH_PCT, max_left.max(MIN_LEFT_WIDTH_PCT))
}

fn clamp_right_width_pct(right_width_pct: u16, left_width_pct: u16) -> u16 {
    let max_right = MAX_RIGHT_WIDTH_PCT.min(100 - MIN_CENTER_WIDTH_PCT - left_width_pct);
    right_width_pct.clamp(MIN_RIGHT_WIDTH_PCT, max_right.max(MIN_RIGHT_WIDTH_PCT))
}

fn pct_from_columns(columns: u16, total_width: u16) -> u16 {
    if total_width == 0 {
        return 0;
    }

    (((u32::from(columns) * 100) + (u32::from(total_width) / 2)) / u32::from(total_width)) as u16
}

fn encode_cursor_key(code: KeyCode, term_mode: TermMode) -> &'static [u8] {
    let application_cursor = term_mode.contains(TermMode::APP_CURSOR);
    match (code, application_cursor) {
        (KeyCode::Up, true) => b"\x1bOA",
        (KeyCode::Down, true) => b"\x1bOB",
        (KeyCode::Right, true) => b"\x1bOC",
        (KeyCode::Left, true) => b"\x1bOD",
        (KeyCode::Up, false) => b"\x1b[A",
        (KeyCode::Down, false) => b"\x1b[B",
        (KeyCode::Right, false) => b"\x1b[C",
        (KeyCode::Left, false) => b"\x1b[D",
        _ => b"",
    }
}

fn agent_wheel_route(term_mode: TermMode) -> AgentWheelRoute {
    let mouse_reporting = term_mode
        .intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION);
    let modern_mouse_encoding =
        term_mode.contains(TermMode::SGR_MOUSE) || term_mode.contains(TermMode::UTF8_MOUSE);

    if mouse_reporting && modern_mouse_encoding {
        AgentWheelRoute::ForwardMouse
    } else if term_mode.contains(TermMode::ALT_SCREEN)
        && term_mode.contains(TermMode::ALTERNATE_SCROLL)
    {
        AgentWheelRoute::ForwardAlternateScroll
    } else {
        AgentWheelRoute::HostScrollback
    }
}

fn push_mouse_codepoint(bytes: &mut Vec<u8>, value: u32) -> Option<()> {
    let ch = char::from_u32(value)?;
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    Some(())
}

fn encode_mouse_scroll(mouse: MouseEvent, area: Rect, term_mode: TermMode) -> Option<Vec<u8>> {
    let mut cb = match mouse.kind {
        MouseEventKind::ScrollUp => 64u16,
        MouseEventKind::ScrollDown => 65u16,
        MouseEventKind::ScrollLeft => 66u16,
        MouseEventKind::ScrollRight => 67u16,
        _ => return None,
    };

    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        cb += 4;
    }
    if mouse.modifiers.contains(KeyModifiers::ALT) {
        cb += 8;
    }
    if mouse.modifiers.contains(KeyModifiers::CONTROL) {
        cb += 16;
    }

    let column = u32::from(mouse.column.saturating_sub(area.x).saturating_add(1));
    let row = u32::from(mouse.row.saturating_sub(area.y).saturating_add(1));

    if term_mode.contains(TermMode::SGR_MOUSE) {
        return Some(format!("\x1b[<{cb};{column};{row}M").into_bytes());
    }

    if term_mode.contains(TermMode::UTF8_MOUSE) {
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(b"\x1b[M");
        push_mouse_codepoint(&mut bytes, u32::from(cb) + 32)?;
        push_mouse_codepoint(&mut bytes, column + 32)?;
        push_mouse_codepoint(&mut bytes, row + 32)?;
        return Some(bytes);
    }

    let cb = u8::try_from(cb + 32).ok()?;
    let column = u8::try_from(column + 32).ok()?;
    let row = u8::try_from(row + 32).ok()?;
    Some(vec![0x1b, b'[', b'M', cb, column, row])
}

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        // Prompts take precedence over every other input target so modal text
        // fields can safely capture keystrokes even when other modes were
        // previously active.
        if !matches!(self.prompt, PromptState::None) {
            return self.handle_prompt_key(key);
        }
        // Interactive mode: ALL remaining keys go to the PTY except Ctrl-G
        // (ExitInteractive, handled inside handle_agent_input). This must
        // happen before global bindings so CloseOverlay / Escape do not
        // intercept agent input during interactive mode.
        if self.input_target == InputTarget::Agent {
            return self.handle_agent_input(key);
        }
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
        if let Some(action) = self.bindings.lookup(&key, BindingScope::Global) {
            match action {
                Action::Quit => {
                    let active_count = self.providers.len();
                    if active_count > 0 {
                        self.prompt = PromptState::ConfirmQuit {
                            active_count,
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
                Action::OpenPalette => {
                    self.prompt = PromptState::Command {
                        input: String::new(),
                        selected: 0,
                        searching: false,
                    };
                    self.set_info("Command palette opened.");
                }
                Action::FocusNext => {
                    let has_staged = !self.staged_files.is_empty();
                    if self.focus == FocusPane::Files {
                        match self.right_section.next(has_staged) {
                            Some(next) => {
                                self.right_section = next;
                                self.clamp_files_cursor();
                            }
                            None => {
                                self.focus = self.focus.next();
                            }
                        }
                    } else {
                        self.focus = self.focus.next();
                        if self.focus == FocusPane::Files {
                            self.right_section = RightSection::first();
                            self.clamp_files_cursor();
                        }
                    }
                    self.input_target = InputTarget::None;
                    self.fullscreen_agent = false;
                }
                Action::FocusPrev => {
                    let has_staged = !self.staged_files.is_empty();
                    if self.focus == FocusPane::Files {
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
                        if self.focus == FocusPane::Files {
                            self.right_section = RightSection::last(has_staged);
                            self.clamp_files_cursor();
                        }
                    }
                    self.input_target = InputTarget::None;
                    self.fullscreen_agent = false;
                }
                Action::ToggleSidebar => {
                    self.left_collapsed = !self.left_collapsed;
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
                        self.set_info("Resize mode off.");
                    }
                }
                _ => {}
            }
            return Ok(false);
        }
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
        let item_count = self.left_items().len();
        if let Some(action) = self.bindings.lookup(&key, BindingScope::Left) {
            match action {
                Action::MoveDown => {
                    if self.selected_left + 1 < item_count {
                        self.selected_left += 1;
                        self.reload_changed_files();
                    }
                }
                Action::MoveUp => {
                    if self.selected_left > 0 {
                        self.selected_left -= 1;
                        self.reload_changed_files();
                    }
                }
                Action::FocusAgent => self.activate_selected_left_item()?,
                Action::OpenProjectBrowser => {
                    self.open_project_browser()?;
                }
                Action::NewAgent => self.create_agent_for_selected_project()?,
                Action::RefreshProject => self.refresh_selected_project()?,
                Action::DeleteSession => self.confirm_delete_selected_session()?,
                Action::RenameSession => self.open_rename_session()?,
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
                        self.fullscreen_agent = true;
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

    fn handle_center_key(&mut self, key: KeyEvent) -> Result<()> {
        let in_diff = matches!(self.center_mode, CenterMode::Diff { .. });
        if let Some(action) = self.bindings.lookup(&key, BindingScope::Center) {
            match action {
                Action::FocusAgent if !in_diff => self.activate_center_agent()?,
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
                        self.fullscreen_agent = true;
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
                _ => {}
            }
        }
        Ok(())
    }

    fn scroll_pty(&mut self, direction: ScrollDirection, amount: usize) {
        let sid = match self.selected_session() {
            Some(s) => s.id.clone(),
            None => return,
        };
        let provider = match self.providers.get(&sid) {
            Some(p) => p,
            None => return,
        };
        let up = matches!(direction, ScrollDirection::Up);
        provider.scroll(up, amount);
    }

    fn reset_pty_scrollback(&self) {
        if let Some(session) = self.selected_session()
            && let Some(provider) = self.providers.get(&session.id)
        {
            provider.set_scrollback(0);
        }
    }

    fn handle_files_key(&mut self, key: KeyEvent) -> Result<()> {
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
                _ => {}
            }
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
        match key.code {
            KeyCode::Enter => {
                self.commit_input.insert(self.commit_input_cursor, '\n');
                self.commit_input_cursor += 1;
            }
            KeyCode::Char(ch) => {
                self.commit_input.insert(self.commit_input_cursor, ch);
                self.commit_input_cursor += ch.len_utf8();
            }
            KeyCode::Backspace => {
                if self.commit_input_cursor > 0 {
                    let prev = self.commit_input[..self.commit_input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.commit_input.remove(prev);
                    self.commit_input_cursor = prev;
                }
            }
            KeyCode::Left => {
                if self.commit_input_cursor > 0 {
                    self.commit_input_cursor = self.commit_input[..self.commit_input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            }
            KeyCode::Right => {
                if self.commit_input_cursor < self.commit_input.len() {
                    self.commit_input_cursor = self.commit_input[self.commit_input_cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.commit_input_cursor + i)
                        .unwrap_or(self.commit_input.len());
                }
            }
            _ => {}
        }
        Ok(())
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
        if self.commit_generating {
            return Ok(());
        }
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let worktree = PathBuf::from(&session.worktree_path);
        let project_path = session.project_path.as_deref().unwrap_or("");
        let prompt = self.config.commit_prompt_for_project(project_path);
        let cfg = provider_config(&self.config, &session.provider);
        let prov = provider::create_provider(session.provider.as_str(), cfg);
        let tx = self.worker_tx.clone();
        self.commit_generating = true;
        self.set_busy("Generating AI commit message from staged diff…");
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
        if self.commit_input.trim().is_empty() {
            self.set_error("Enter a commit message first.");
            return Ok(());
        }
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let worktree = PathBuf::from(&session.worktree_path);
        match git::commit(&worktree, &self.commit_input) {
            Ok(_) => {
                self.commit_input.clear();
                self.commit_input_cursor = 0;
                self.commit_scroll = 0;
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

    fn pull_from_remote(&mut self) -> Result<()> {
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let worktree = PathBuf::from(&session.worktree_path);
        let tx = self.worker_tx.clone();
        self.set_busy("Pulling latest changes from remote…");
        thread::spawn(move || {
            let result = git::pull_current_branch(&worktree)
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = tx.send(WorkerEvent::PullCompleted(result));
        });
        Ok(())
    }

    fn handle_agent_input(&mut self, key: KeyEvent) -> Result<bool> {
        let session_id = match self.selected_session() {
            Some(s) => s.id.clone(),
            None => {
                self.input_target = InputTarget::None;
                return Ok(false);
            }
        };
        let provider = match self.providers.get(&session_id) {
            Some(p) => p,
            None => {
                self.input_target = InputTarget::None;
                self.set_error("Agent disconnected.");
                return Ok(false);
            }
        };

        // Exit interactive mode via configured binding (default: ctrl-g).
        if let Some(Action::ExitInteractive) = self.bindings.lookup(&key, BindingScope::Interactive)
        {
            self.input_target = InputTarget::None;
            self.fullscreen_agent = false;
            let reenter_key = self.bindings.label_for(Action::FocusAgent);
            self.set_info(format!(
                "Exited interactive mode. Press {reenter_key} to re-enter."
            ));
            return Ok(false);
        }

        // Scroll bindings are checked before forwarding keys to the PTY so
        // that the configured keys for ScrollPageUp, ScrollPageDown,
        // ScrollLineUp, and ScrollLineDown work in interactive mode without
        // being eaten by the child process.
        match self.bindings.lookup(&key, BindingScope::Interactive) {
            Some(Action::ScrollPageUp) => {
                if self.last_pty_size.0 > 0 {
                    self.scroll_pty(ScrollDirection::Up, self.last_pty_size.0 as usize);
                }
                return Ok(false);
            }
            Some(Action::ScrollPageDown) => {
                if self.last_pty_size.0 > 0 {
                    self.scroll_pty(ScrollDirection::Down, self.last_pty_size.0 as usize);
                }
                return Ok(false);
            }
            Some(Action::ScrollLineUp)
                if provider.scrollback_offset() > 0 && self.last_pty_size.0 > 0 =>
            {
                self.scroll_pty(ScrollDirection::Up, 1);
                return Ok(false);
            }
            Some(Action::ScrollLineDown)
                if provider.scrollback_offset() > 0 && self.last_pty_size.0 > 0 =>
            {
                self.scroll_pty(ScrollDirection::Down, 1);
                return Ok(false);
            }
            _ => {}
        }

        match key.code {
            KeyCode::Esc => {
                let _ = provider.write_bytes(b"\x1b");
            }
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Map ctrl+a..=ctrl+z to the corresponding control character (0x01..=0x1a).
                if c.is_ascii_lowercase() {
                    let ctrl_byte = c as u8 - b'a' + 1;
                    let _ = provider.write_bytes(&[ctrl_byte]);
                }
            }
            KeyCode::Char(c) => {
                let mut buf = [0u8; 4];
                let bytes = c.encode_utf8(&mut buf);
                let _ = provider.write_bytes(bytes.as_bytes());
            }
            KeyCode::Enter => {
                let _ = provider.write_bytes(b"\r");
            }
            KeyCode::Backspace => {
                let _ = provider.write_bytes(b"\x7f");
            }
            KeyCode::Tab => {
                let _ = provider.write_bytes(b"\t");
            }
            KeyCode::Up => {
                let _ = provider.write_bytes(encode_cursor_key(KeyCode::Up, provider.term_mode()));
            }
            KeyCode::Down => {
                let _ =
                    provider.write_bytes(encode_cursor_key(KeyCode::Down, provider.term_mode()));
            }
            KeyCode::Right => {
                let _ =
                    provider.write_bytes(encode_cursor_key(KeyCode::Right, provider.term_mode()));
            }
            KeyCode::Left => {
                let _ =
                    provider.write_bytes(encode_cursor_key(KeyCode::Left, provider.term_mode()));
            }
            KeyCode::Home => {
                let _ = provider.write_bytes(b"\x1b[H");
            }
            KeyCode::End => {
                let _ = provider.write_bytes(b"\x1b[F");
            }
            KeyCode::Delete => {
                let _ = provider.write_bytes(b"\x1b[3~");
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        if matches!(self.prompt, PromptState::Command { .. }) {
            // Determine search state and do binding lookup before taking a mutable borrow.
            let is_searching = matches!(
                self.prompt,
                PromptState::Command {
                    searching: true,
                    ..
                }
            );
            let is_plain_char = matches!(key.code, KeyCode::Char(_))
                && !key.modifiers.contains(KeyModifiers::CONTROL);
            let action = if is_searching && is_plain_char {
                None
            } else {
                self.bindings.lookup(&key, BindingScope::Palette)
            };

            match action {
                Some(Action::CloseOverlay) => {
                    if is_searching {
                        if let PromptState::Command { searching, .. } = &mut self.prompt {
                            *searching = false;
                        }
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                Some(Action::SearchToggle) if !is_searching => {
                    if let PromptState::Command { searching, .. } = &mut self.prompt {
                        *searching = true;
                    }
                }
                Some(Action::MoveDown) => {
                    if let PromptState::Command {
                        input, selected, ..
                    } = &mut self.prompt
                    {
                        let count = self.bindings.filtered_palette(input).len();
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
                    if is_searching {
                        if let PromptState::Command { searching, .. } = &mut self.prompt {
                            *searching = false;
                        }
                    } else {
                        let command = if let PromptState::Command {
                            input, selected, ..
                        } = &self.prompt
                        {
                            if let Some(binding) =
                                self.bindings.filtered_palette(input).get(*selected)
                            {
                                binding.palette_name.unwrap().to_string()
                            } else {
                                input.trim().to_string()
                            }
                        } else {
                            String::new()
                        };
                        self.prompt = PromptState::None;
                        if let Err(e) = self.execute_command(command) {
                            self.set_error(format!("{e:#}"));
                        }
                    }
                }
                _ => {
                    // Text input fallback: Tab (autocomplete), Backspace, Char.
                    if let PromptState::Command {
                        input, selected, ..
                    } = &mut self.prompt
                    {
                        match key.code {
                            KeyCode::Tab => {
                                if let Some(binding) =
                                    self.bindings.filtered_palette(input).get(*selected)
                                {
                                    *input = binding.palette_name.unwrap().to_string();
                                    *selected = 0;
                                }
                            }
                            KeyCode::Backspace => {
                                input.pop();
                                *selected = 0;
                            }
                            KeyCode::Char(c) if is_plain_char => {
                                input.push(c);
                                *selected = 0;
                            }
                            _ => {}
                        }
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
                        KeyCode::Backspace => {
                            path_input.pop();
                            tab_completions.clear();
                            *tab_index = 0;
                        }
                        KeyCode::Tab | KeyCode::BackTab => {
                            if tab_completions.is_empty() {
                                let input_path = PathBuf::from(path_input.as_str());
                                let (search_dir, prefix) =
                                    if input_path.is_dir() && path_input.ends_with('/') {
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
                                *path_input = completion.clone();
                            }
                        }
                        KeyCode::Enter => {
                            let new_dir = PathBuf::from(path_input.trim());
                            if new_dir.is_dir() {
                                *current_dir = new_dir.clone();
                                entries.clear();
                                *loading = true;
                                *selected = 0;
                                filter.clear();
                                browse_to = Some(new_dir);
                            } else {
                                error_msg =
                                    Some(format!("{} is not a directory.", path_input.trim()));
                            }
                            *editing_path = false;
                            path_input.clear();
                            tab_completions.clear();
                            *tab_index = 0;
                        }
                        KeyCode::Char(c) if is_plain_char => {
                            path_input.push(c);
                            tab_completions.clear();
                            *tab_index = 0;
                        }
                        _ => {}
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

            let mut browse_to: Option<PathBuf> = None;
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
                    if let PromptState::BrowseProjects { searching, .. } = &mut self.prompt {
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
                            let needle = filter.to_lowercase();
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
                        *path_input = p;
                    }
                }
                Some(Action::Confirm) if is_searching => {
                    if let PromptState::BrowseProjects { searching, .. } = &mut self.prompt {
                        *searching = false;
                    }
                }
                Some(Action::OpenEntry) if !is_searching => {
                    if let PromptState::BrowseProjects {
                        current_dir,
                        entries,
                        loading,
                        selected,
                        filter,
                        ..
                    } = &mut self.prompt
                    {
                        let visible: Vec<_> = if filter.is_empty() {
                            entries.iter().collect()
                        } else {
                            let needle = filter.to_lowercase();
                            entries
                                .iter()
                                .filter(|e| e.label.to_lowercase().contains(&needle))
                                .collect()
                        };
                        if let Some(entry) = visible.get(*selected).cloned() {
                            let new_dir = entry.path.clone();
                            *current_dir = new_dir.clone();
                            entries.clear();
                            *loading = true;
                            *selected = 0;
                            filter.clear();
                            browse_to = Some(new_dir);
                        }
                    }
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
                    {
                        match key.code {
                            KeyCode::Backspace => {
                                filter.pop();
                                *selected = 0;
                            }
                            KeyCode::Char(c) if is_plain_char => {
                                filter.push(c);
                                *selected = 0;
                            }
                            _ => {}
                        }
                    }
                }
            }
            if let Some(dir) = browse_to {
                self.spawn_browser_entries(&dir);
            }
            return Ok(false);
        }

        if let PromptState::PickEditor {
            session_label,
            worktree_path,
            editors,
            selected,
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
                    let editor = editors.get(*selected).cloned();
                    let worktree = worktree_path.clone();
                    let label = session_label.clone();
                    self.prompt = PromptState::None;
                    if let Some(editor) = editor
                        && let Err(e) = self.open_worktree_in_editor(&worktree, &label, &editor)
                    {
                        self.set_error(format!("{e:#}"));
                    }
                }
                _ => {}
            }
            return Ok(false);
        }

        if let PromptState::ConfirmDeleteAgent {
            session_id,
            confirm_selected,
            ..
        } = &mut self.prompt
        {
            match self.bindings.lookup(&key, BindingScope::Dialog) {
                Some(Action::CloseOverlay) => self.prompt = PromptState::None,
                Some(Action::ToggleSelection) => {
                    *confirm_selected = !*confirm_selected;
                }
                Some(Action::Confirm) => {
                    if *confirm_selected {
                        let id = session_id.clone();
                        self.prompt = PromptState::None;
                        if let Err(e) = self.do_delete_session(&id) {
                            self.set_error(format!("{e:#}"));
                        }
                    } else {
                        self.prompt = PromptState::None;
                    }
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
                    if *confirm_selected {
                        return Ok(true);
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                _ => {}
            }
        }

        if let PromptState::ConfirmDiscardFile {
            file_path,
            is_untracked,
            confirm_selected,
        } = &mut self.prompt
        {
            match self.bindings.lookup(&key, BindingScope::Dialog) {
                Some(Action::CloseOverlay) => self.prompt = PromptState::None,
                Some(Action::ToggleSelection) => {
                    *confirm_selected = !*confirm_selected;
                }
                Some(Action::Confirm) => {
                    if *confirm_selected {
                        let fp = file_path.clone();
                        let ut = *is_untracked;
                        self.prompt = PromptState::None;
                        if let Some(session) = self.selected_session() {
                            let worktree = PathBuf::from(&session.worktree_path);
                            match git::discard_file(&worktree, &fp, ut) {
                                Ok(()) => {
                                    self.set_info(format!("Discarded changes to \"{fp}\". File restored to last committed state."));
                                    self.reload_changed_files();
                                }
                                Err(e) => self.set_error(format!("Discard failed: {e}")),
                            }
                        }
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                _ => {}
            }
            return Ok(false);
        }

        if let PromptState::RenameSession {
            session_id,
            input,
            cursor,
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
                    let new_name = input.clone();
                    self.prompt = PromptState::None;
                    self.apply_rename_session(&id, new_name);
                }
                _ => match key.code {
                    KeyCode::Backspace => {
                        if *cursor > 0 {
                            input.remove(*cursor - 1);
                            *cursor -= 1;
                        }
                    }
                    KeyCode::Delete => {
                        if *cursor < input.len() {
                            input.remove(*cursor);
                        }
                    }
                    KeyCode::Left => {
                        if *cursor > 0 {
                            *cursor -= 1;
                        }
                    }
                    KeyCode::Right => {
                        if *cursor < input.len() {
                            *cursor += 1;
                        }
                    }
                    KeyCode::Home => {
                        *cursor = 0;
                    }
                    KeyCode::End => {
                        *cursor = input.len();
                    }
                    KeyCode::Char(c) if is_plain_char => {
                        input.insert(*cursor, c);
                        *cursor += 1;
                    }
                    _ => {}
                },
            }
            return Ok(false);
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

    fn mouse_target(&self, column: u16, row: u16) -> Option<MouseTarget> {
        if self.fullscreen_agent {
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

        if column == center_right || column == right_left {
            return Some(ResizeDragState::RightDivider);
        }

        None
    }

    fn set_left_selection(&mut self, index: usize) {
        if index >= self.left_items().len() {
            return;
        }
        self.focus = FocusPane::Left;
        self.input_target = InputTarget::None;
        self.fullscreen_agent = false;
        if self.selected_left != index {
            self.selected_left = index;
            self.reload_changed_files();
        }
    }

    fn register_mouse_click(&mut self, target: MouseClickTarget) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_mouse_click
            && last.target == target
            && now.duration_since(last.at) <= DOUBLE_CLICK_THRESHOLD
        {
            self.last_mouse_click = None;
            return true;
        }

        self.last_mouse_click = Some(RecentMouseClick { target, at: now });
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
                            self.fullscreen_agent = true;
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
                    self.fullscreen_agent = true;
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

    fn activate_center_agent(&mut self) -> Result<()> {
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
            self.fullscreen_agent = true;
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
        self.fullscreen_agent = false;
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
        self.fullscreen_agent = false;
    }

    fn commit_scroll_max(&self) -> u16 {
        let Some(text_area) = self.mouse_layout.commit_text_area else {
            return 0;
        };
        let width = text_area.width as usize;
        if width == 0 {
            return 0;
        }

        let wrapped = wrap_text_at_width(&self.commit_input, width);
        let total_lines = wrapped.split('\n').count() as u16;
        total_lines.saturating_sub(text_area.height)
    }

    fn set_commit_cursor_from_mouse(&mut self, column: u16, row: u16) {
        let Some(text_area) = self.mouse_layout.commit_text_area else {
            self.commit_input_cursor = self.commit_input.len();
            return;
        };
        let width = text_area.width as usize;
        if width == 0 {
            self.commit_input_cursor = self.commit_input.len();
            return;
        }

        let relative_row = row
            .saturating_sub(text_area.y)
            .saturating_add(self.commit_scroll);
        let relative_col = usize::from(column.saturating_sub(text_area.x));
        self.commit_input_cursor =
            cursor_from_wrapped_position(&self.commit_input, width, relative_row, relative_col);
    }

    fn scroll_commit_input(&mut self, down: bool) {
        let max_scroll = self.commit_scroll_max();
        if down {
            self.commit_scroll = (self.commit_scroll + MOUSE_WHEEL_LINES as u16).min(max_scroll);
        } else {
            self.commit_scroll = self.commit_scroll.saturating_sub(MOUSE_WHEEL_LINES as u16);
        }
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

        let Some(area) = self.mouse_layout.agent_term else {
            return;
        };
        let Some(session_id) = self.selected_session().map(|session| session.id.clone()) else {
            return;
        };
        let Some(provider) = self.providers.get(&session_id) else {
            return;
        };

        match agent_wheel_route(provider.term_mode()) {
            AgentWheelRoute::HostScrollback => {
                provider.scroll(
                    matches!(mouse.kind, MouseEventKind::ScrollUp),
                    MOUSE_WHEEL_LINES,
                );
            }
            AgentWheelRoute::ForwardMouse => {
                provider.set_scrollback(0);
                if let Some(bytes) = encode_mouse_scroll(mouse, area, provider.term_mode()) {
                    let _ = provider.write_bytes(&bytes);
                }
            }
            AgentWheelRoute::ForwardAlternateScroll => {
                provider.set_scrollback(0);
                let code = match mouse.kind {
                    MouseEventKind::ScrollUp => KeyCode::Up,
                    MouseEventKind::ScrollDown => KeyCode::Down,
                    _ => return,
                };
                let _ = provider.write_bytes(encode_cursor_key(code, provider.term_mode()));
            }
        }
    }

    fn update_dragged_widths(&mut self, column: u16) {
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
            None => {}
        }
    }

    fn persist_pane_widths(&mut self) {
        if self.config.ui.left_width_pct != self.left_width_pct
            || self.config.ui.right_width_pct != self.right_width_pct
        {
            self.config.ui.left_width_pct = self.left_width_pct;
            self.config.ui.right_width_pct = self.right_width_pct;
            let _ = save_config(&self.paths.config_path, &self.config, &self.bindings);
        }
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        if !matches!(self.prompt, PromptState::None) {
            return;
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
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(drag) = self.resize_drag_at_mouse(mouse.column, mouse.row) {
                    self.mouse_drag = Some(drag);
                    self.update_dragged_widths(mouse.column);
                    return;
                }

                match self.mouse_target(mouse.column, mouse.row) {
                    Some(MouseTarget::LeftPane) => {
                        self.focus = FocusPane::Left;
                        self.input_target = InputTarget::None;
                        self.fullscreen_agent = false;
                    }
                    Some(MouseTarget::LeftRow(index)) => {
                        let double_click =
                            self.register_mouse_click(MouseClickTarget::LeftRow(index));
                        self.set_left_selection(index);
                        if double_click {
                            match self.left_items().get(self.selected_left) {
                                Some(LeftItem::Project(_)) => {
                                    self.toggle_collapse_selected_project()
                                }
                                Some(LeftItem::Session(_)) => {
                                    self.activate_selected_left_item_from_mouse()
                                }
                                None => {}
                            }
                        }
                    }
                    Some(MouseTarget::Center) => {
                        let double_click = self.register_mouse_click(MouseClickTarget::CenterPane);
                        self.focus = FocusPane::Center;
                        if double_click {
                            self.activate_center_agent_from_mouse();
                        }
                    }
                    Some(MouseTarget::FilesPane) => {
                        self.focus = FocusPane::Files;
                        self.input_target = InputTarget::None;
                        self.fullscreen_agent = false;
                    }
                    Some(MouseTarget::UnstagedFile(index)) => {
                        let double_click = index
                            .map(|i| self.register_mouse_click(MouseClickTarget::UnstagedFile(i)));
                        self.set_file_selection(RightSection::Unstaged, index);
                        if matches!(double_click, Some(true)) {
                            self.open_selected_file_diff_from_mouse();
                        }
                    }
                    Some(MouseTarget::StagedFile(index)) => {
                        let double_click = index
                            .map(|i| self.register_mouse_click(MouseClickTarget::StagedFile(i)));
                        self.set_file_selection(RightSection::Staged, index);
                        if matches!(double_click, Some(true)) {
                            self.open_selected_file_diff_from_mouse();
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
                    self.update_dragged_widths(mouse.column);
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
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, mpsc};

    use crate::app::{
        App, CenterMode, FocusPane, InputTarget, MouseLayoutState, PromptState, RightSection,
    };
    use crate::config::{Config, DuxPaths};
    use crate::keybindings::{Action, BINDING_DEFS, BindingScope, RuntimeBindings};
    use crate::model::{AgentSession, ChangedFile, Project, ProviderKind, SessionStatus};
    use crate::pty::PtyClient;
    use crate::statusline::StatusLine;
    use crate::storage::SessionStore;
    use crate::theme::Theme;
    use alacritty_terminal::term::TermMode;
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
            right_section: RightSection::Unstaged,
            files_index: 0,
            commit_input: String::new(),
            commit_input_cursor: 0,
            commit_scroll: 0,
            commit_generating: false,
            left_width_pct: 20,
            right_width_pct: 23,
            focus: FocusPane::Left,
            center_mode: CenterMode::Agent,
            left_collapsed: false,
            resize_mode: false,
            help_scroll: None,
            last_help_height: 0,
            last_help_lines: 0,
            fullscreen_agent: false,
            status: StatusLine::new("ready"),
            prompt: PromptState::None,
            input_target: InputTarget::None,
            worker_tx,
            worker_rx,
            providers: std::collections::HashMap::new(),
            create_agent_in_flight: false,
            last_pty_size: (0, 0),
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme: Theme::default_dark(),
            tick_count: 0,
            watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
            has_active_agent: Arc::new(AtomicBool::new(false)),
            collapsed_projects: std::collections::HashSet::new(),
            left_items_cache: Vec::new(),
            mouse_layout: MouseLayoutState::default(),
            mouse_drag: None,
            last_mouse_click: None,
        };
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
            agent_term: Some(Rect::new(21, 1, 55, 16)),
            unstaged_list: Some(Rect::new(78, 1, 21, 8)),
            staged_list: Some(Rect::new(78, 9, 21, 5)),
            commit_area: Some(Rect::new(77, 14, 23, 6)),
            commit_text_area: Some(Rect::new(78, 15, 21, 4)),
        };
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
    fn rename_session_prompt_accepts_text_before_agent_input() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: "agent".to_string(),
            cursor: 5,
        };
        app.input_target = InputTarget::Agent;

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::RenameSession { input, cursor, .. } => {
                assert_eq!(input, "agentx");
                assert_eq!(*cursor, 6);
            }
            other => panic!("expected rename prompt, got {other:?}"),
        }
    }

    #[test]
    fn rename_session_text_ignores_printable_close_overlay_binding() {
        let mut app = test_app(bindings_with_overrides(&[(Action::CloseOverlay, &["x"])]));
        app.prompt = PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: "agent".to_string(),
            cursor: 5,
        };

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();

        match &app.prompt {
            PromptState::RenameSession { input, cursor, .. } => {
                assert_eq!(input, "agentx");
                assert_eq!(*cursor, 6);
            }
            other => panic!("expected rename prompt, got {other:?}"),
        }
    }

    #[test]
    fn rename_session_uses_custom_dialog_confirm_binding() {
        let mut app = test_app(bindings_with_overrides(&[(Action::Confirm, &["tab"])]));
        app.prompt = PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: "agent-branch".to_string(),
            cursor: "agent-branch".len(),
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
        app.fullscreen_agent = true;

        app.open_rename_session().unwrap();

        assert!(matches!(app.prompt, PromptState::RenameSession { .. }));
        assert_eq!(app.input_target, InputTarget::None);
        assert!(!app.fullscreen_agent);
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
            },
            ChangedFile {
                path: "b.txt".into(),
                status: "M".into(),
                additions: 2,
                deletions: 1,
            },
        ];
        app.focus = FocusPane::Center;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 2));

        assert_eq!(app.focus, FocusPane::Files);
        assert_eq!(app.right_section, RightSection::Unstaged);
        assert_eq!(app.files_index, 1);
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
    fn mouse_double_click_project_row_toggles_collapse_like_space() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.selected_left = 0;
        app.focus = FocusPane::Left;
        let project_id = app.projects[0].id.clone();

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1));

        assert_eq!(app.focus, FocusPane::Left);
        assert!(app.collapsed_projects.contains(&project_id));
        assert_eq!(app.selected_left, 0);
        assert_eq!(app.input_target, InputTarget::None);
        assert!(!app.fullscreen_agent);
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
        assert!(!app.fullscreen_agent);
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
        assert!(app.fullscreen_agent);
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
        }];
        app.staged_files = vec![ChangedFile {
            path: "b.txt".into(),
            status: "A".into(),
            additions: 3,
            deletions: 0,
        }];

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 1));
        assert_eq!(app.right_section, RightSection::Unstaged);
        assert_eq!(app.files_index, 0);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 9));
        assert_eq!(app.right_section, RightSection::Staged);
        assert_eq!(app.files_index, 0);
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
        }];
        app.selected_left = 1;
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 1));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 79, 1));

        assert_eq!(app.focus, FocusPane::Center);
        assert!(matches!(app.center_mode, CenterMode::Diff { .. }));
    }

    #[test]
    fn mouse_wheel_center_diff_scrolls_lines() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.center_mode = CenterMode::Diff {
            lines: vec![Line::from("one"), Line::from("two"), Line::from("three")],
            scroll: 0,
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
        app.commit_input = "hello world".to_string();
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
        app.commit_input = "abc\ndef".to_string();
        app.commit_input_cursor = 0;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 16));
        assert_eq!(app.input_target, InputTarget::None);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 16));

        assert_eq!(app.input_target, InputTarget::CommitMessage);
        assert!(app.commit_input_cursor > 0);
    }

    #[test]
    fn mouse_journey_commit_chrome_then_text_enters_editing() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.commit_input = "hello".to_string();
        app.focus = FocusPane::Center;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 14));
        assert_eq!(app.focus, FocusPane::Files);
        assert_eq!(app.right_section, RightSection::CommitInput);
        assert_eq!(app.input_target, InputTarget::None);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 80, 15));
        assert_eq!(app.input_target, InputTarget::CommitMessage);
    }

    #[test]
    fn mouse_wheel_commit_text_scrolls_commit_viewport() {
        let mut app = test_app(default_bindings());
        install_mouse_layout(&mut app);
        app.commit_input = (0..20).map(|i| format!("line {i}\n")).collect::<String>();
        app.commit_scroll = 0;

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 80, 15));

        assert!(app.commit_scroll > 0);
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
    fn agent_wheel_route_prefers_mouse_reporting_with_modern_encoding() {
        let mode = TermMode::ALT_SCREEN | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            super::agent_wheel_route(mode),
            super::AgentWheelRoute::ForwardMouse
        );
    }

    #[test]
    fn agent_wheel_route_uses_alternate_scroll_without_modern_encoding() {
        let mode = TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL | TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            super::agent_wheel_route(mode),
            super::AgentWheelRoute::ForwardAlternateScroll
        );
    }

    #[test]
    fn agent_wheel_route_falls_back_to_host_scrollback_without_modern_encoding() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            super::agent_wheel_route(mode),
            super::AgentWheelRoute::HostScrollback
        );
    }
}
