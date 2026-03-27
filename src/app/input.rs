use super::*;

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if self.bindings.lookup(&key, BindingScope::Global) == Some(Action::CloseOverlay)
            && self.close_top_overlay()
        {
            return Ok(false);
        }
        if let Some(ref mut scroll) = self.help_scroll {
            // Help overlay is open — consume all keys, only scroll keys do anything.
            if let Some(action) = self.bindings.lookup(&key, BindingScope::Left) {
                match action {
                    Action::MoveDown => *scroll = scroll.saturating_add(1),
                    Action::MoveUp => *scroll = scroll.saturating_sub(1),
                    _ => {}
                }
            }
            if let Some(action) = self.bindings.lookup(&key, BindingScope::Global) {
                match action {
                    Action::ScrollPageDown => {
                        let page = self.last_help_height.max(1);
                        *scroll = (*scroll + page).min(
                            self.last_help_lines
                                .saturating_sub(self.last_help_height.max(1)),
                        );
                    }
                    Action::ScrollPageUp => {
                        let page = self.last_help_height.max(1);
                        *scroll = scroll.saturating_sub(page);
                    }
                    _ => {}
                }
            }
            return Ok(false);
        }
        if !matches!(self.prompt, PromptState::None) {
            return self.handle_prompt_key(key);
        }
        // In interactive mode, ALL keys go to the PTY except ctrl+g (handled
        // inside handle_agent_input). This must be checked before quit / help /
        // palette so that typing 'q', ctrl+c, '?', ctrl+p etc. reaches the CLI.
        if self.input_target == InputTarget::Agent {
            return self.handle_agent_input(key);
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
                Action::FocusAgent => match self.left_items().get(self.selected_left) {
                    Some(LeftItem::Project(project_index)) => {
                        let project_id = self.projects[*project_index].id.clone();
                        let has_sessions = self.sessions.iter().any(|s| s.project_id == project_id);
                        if has_sessions {
                            if self.collapsed_projects.contains(&project_id) {
                                self.collapsed_projects.remove(&project_id);
                                self.rebuild_left_items();
                            }
                            if let Some(pos) =
                                    self.left_items().iter().position(|item| {
                                        matches!(item, LeftItem::Session(si) if self.sessions[*si].project_id == project_id)
                                    })
                                {
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
                        } else if self.selected_session().is_some() {
                            self.reconnect_selected_session()?;
                        }
                    }
                    None => {}
                },
                Action::OpenProjectBrowser => {
                    self.open_project_browser()?;
                }
                Action::NewAgent => self.create_agent_for_selected_project()?,
                Action::RefreshProject => self.refresh_selected_project()?,
                Action::DeleteSession => self.confirm_delete_selected_session()?,
                Action::RenameSession => self.open_rename_session()?,
                Action::CycleProvider => self.cycle_selected_project_provider()?,
                Action::CopyPath => self.copy_selected_path()?,
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
                Action::InteractAgent if !in_diff => {
                    if self.selected_session().is_some()
                        && self
                            .selected_session()
                            .map(|s| self.providers.contains_key(&s.id))
                            .unwrap_or(false)
                    {
                        self.reset_pty_scrollback();
                        self.input_target = InputTarget::Agent;
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
                    } else if self.selected_session().is_some() {
                        self.reconnect_selected_session()?;
                    }
                }
                Action::ToggleFullscreen if !in_diff => {
                    self.fullscreen_agent = !self.fullscreen_agent;
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
        if let Some(session) = self.selected_session() {
            if let Some(provider) = self.providers.get(&session.id) {
                provider.set_scrollback(0);
            }
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
            self.set_info("Exited interactive mode.");
            return Ok(false);
        }

        // Toggle fullscreen overlay (default: ctrl-e) without leaving interactive mode.
        if let Some(Action::ToggleFullscreen) =
            self.bindings.lookup(&key, BindingScope::Interactive)
        {
            self.fullscreen_agent = !self.fullscreen_agent;
            return Ok(false);
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
                let _ = provider.write_bytes(b"\x1b[A");
            }
            KeyCode::Down => {
                let _ = provider.write_bytes(b"\x1b[B");
            }
            KeyCode::Right => {
                let _ = provider.write_bytes(b"\x1b[C");
            }
            KeyCode::Left => {
                let _ = provider.write_bytes(b"\x1b[D");
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
            KeyCode::PageUp => {
                let _ = provider.write_bytes(b"\x1b[5~");
            }
            KeyCode::PageDown => {
                let _ = provider.write_bytes(b"\x1b[6~");
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
                    if let PromptState::Command { selected, .. } = &mut self.prompt {
                        if *selected > 0 {
                            *selected -= 1;
                        }
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
                    if let PromptState::BrowseProjects { selected, .. } = &mut self.prompt {
                        if *selected > 0 {
                            *selected -= 1;
                        }
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
                    if is_searching {
                        if let PromptState::BrowseProjects {
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
            }
            if let Some(dir) = browse_to {
                self.spawn_browser_entries(&dir);
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
            match key.code {
                KeyCode::Esc => {
                    self.prompt = PromptState::None;
                }
                KeyCode::Enter => {
                    let id = session_id.clone();
                    let new_name = input.clone();
                    self.prompt = PromptState::None;
                    self.apply_rename_session(&id, new_name);
                }
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
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.insert(*cursor, c);
                    *cursor += 1;
                }
                _ => {}
            }
            return Ok(false);
        }

        Ok(false)
    }

    fn handle_resize_key(&mut self, key: KeyEvent) {
        if let Some(action) = self.bindings.lookup(&key, BindingScope::Resize) {
            if self.focus == FocusPane::Files {
                match action {
                    Action::ResizeShrink => {
                        self.right_width_pct = self.right_width_pct.saturating_add(2).min(50)
                    }
                    Action::ResizeGrow => {
                        self.right_width_pct = self.right_width_pct.saturating_sub(2).max(14)
                    }
                    _ => {}
                }
            } else {
                match action {
                    Action::ResizeShrink => {
                        self.left_width_pct = self.left_width_pct.saturating_sub(2).max(14)
                    }
                    Action::ResizeGrow => {
                        self.left_width_pct = self.left_width_pct.saturating_add(2).min(38)
                    }
                    _ => {}
                }
            }
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
        match mouse.kind {
            MouseEventKind::Down(_) => {
                self.set_info(
                    &format!("Mouse support is available for wheel navigation; resize has a keyboard fallback via {}.", self.bindings.label_for(Action::ToggleResizeMode)),
                );
            }
            MouseEventKind::ScrollDown => match self.focus {
                FocusPane::Left => {
                    let items = self.left_items();
                    if self.selected_left + 1 < items.len() {
                        self.selected_left += 1;
                        self.reload_changed_files();
                    }
                }
                FocusPane::Center => {
                    if let CenterMode::Diff { ref mut scroll, .. } = self.center_mode {
                        let max_scroll = self
                            .last_diff_visual_lines
                            .saturating_sub(self.last_diff_height.max(1));
                        *scroll = (*scroll + 3).min(max_scroll);
                    }
                }
                FocusPane::Files => {
                    if self.files_index + 1 < self.current_files_len() {
                        self.files_index += 1;
                    }
                }
            },
            MouseEventKind::ScrollUp => match self.focus {
                FocusPane::Left => {
                    if self.selected_left > 0 {
                        self.selected_left -= 1;
                        self.reload_changed_files();
                    }
                }
                FocusPane::Center => {
                    if let CenterMode::Diff { ref mut scroll, .. } = self.center_mode {
                        *scroll = scroll.saturating_sub(3);
                    }
                }
                FocusPane::Files => {
                    if self.files_index > 0 {
                        self.files_index -= 1;
                    }
                }
            },
            _ => {}
        }
    }
}
