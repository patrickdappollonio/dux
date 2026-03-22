use super::*;

impl App {
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Esc && self.close_top_overlay() {
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
        if (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
            || key.code == KeyCode::Char('q')
        {
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
        if key.code == KeyCode::Char('?') {
            self.help_overlay = !self.help_overlay;
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            self.prompt = PromptState::Command {
                input: String::new(),
                selected: 0,
                searching: false,
            };
            self.set_info("Command palette opened.");
            return Ok(false);
        }
        if key.code == KeyCode::Tab {
            self.focus = self.focus.next();
            return Ok(false);
        }
        if key.code == KeyCode::BackTab {
            self.focus = self.focus.previous();
            return Ok(false);
        }
        if key.code == KeyCode::Char('[') {
            self.left_collapsed = !self.left_collapsed;
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('w') {
            self.resize_mode = !self.resize_mode;
            if self.resize_mode {
                self.set_info("Resize mode on: h/l/←/→ resize side panes.");
            } else {
                self.persist_pane_widths();
                self.set_info("Resize mode off.");
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
        let items = self.left_items();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected_left + 1 < items.len() {
                    self.selected_left += 1;
                    self.reload_changed_files();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_left > 0 {
                    self.selected_left -= 1;
                    self.reload_changed_files();
                }
            }
            KeyCode::Enter => {
                match self.left_items().get(self.selected_left) {
                    Some(LeftItem::Project(project_index)) => {
                        let project_id = self.projects[*project_index].id.clone();
                        let has_sessions = self.sessions.iter().any(|s| s.project_id == project_id);
                        if has_sessions {
                            // Expand the project if collapsed so the session items are visible
                            if self.collapsed_projects.contains(&project_id) {
                                self.collapsed_projects.remove(&project_id);
                                self.rebuild_left_items();
                            }
                            // Find the first session belonging to this project
                            if let Some(pos) = self.left_items().iter().position(|item| {
                                matches!(item, LeftItem::Session(si) if self.sessions[*si].project_id == project_id)
                            }) {
                                self.selected_left = pos;
                                self.center_mode = CenterMode::Agent;
                                self.focus = FocusPane::Center;
                                self.reload_changed_files();
                                if self.selected_session()
                                    .map(|s| self.providers.contains_key(&s.id))
                                    .unwrap_or(false)
                                {
                                    self.input_target = InputTarget::Agent;
                                }
                            }
                        } else {
                            // Project has no agents: create one
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
                        }
                    }
                    None => {}
                }
            }
            KeyCode::Char('a') => {
                self.open_project_browser()?;
            }
            KeyCode::Char('n') => self.create_agent_for_selected_project()?,
            KeyCode::Char('u') => self.refresh_selected_project()?,
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.confirm_delete_selected_session()?
            }
            KeyCode::Char('d') => self.cycle_selected_project_provider()?,
            KeyCode::Char('r') => self.reconnect_selected_session()?,
            KeyCode::Char('y') => self.copy_selected_path()?,
            KeyCode::Char(' ') => self.toggle_collapse_selected_project(),
            KeyCode::Char('i') => {
                if self.selected_session().is_some()
                    && self
                        .selected_session()
                        .map(|s| self.providers.contains_key(&s.id))
                        .unwrap_or(false)
                {
                    self.focus = FocusPane::Center;
                    self.center_mode = CenterMode::Agent;
                    self.input_target = InputTarget::Agent;
                    self.set_info("Interactive mode. Keys forwarded to agent. ctrl+g exits.");
                } else {
                    self.set_error(
                        "No active agent. Press \"r\" to restart or \"n\" to create a new one.",
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_center_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('i') => {
                if self.selected_session().is_some()
                    && self
                        .selected_session()
                        .map(|s| self.providers.contains_key(&s.id))
                        .unwrap_or(false)
                {
                    self.reset_pty_scrollback();
                    self.input_target = InputTarget::Agent;
                    self.set_info("Interactive mode. Keys forwarded to agent. ctrl+g exits.");
                } else {
                    self.set_error(
                        "No active agent. Press \"r\" to restart or \"n\" to create a new one.",
                    );
                }
            }
            KeyCode::Char('r') | KeyCode::Enter => {
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
            KeyCode::Esc => {
                // When no diff is open, ESC from the center pane is a no-op.
                // Diff closing is handled globally by close_top_overlay so it
                // works regardless of which pane is focused.
            }
            // Page-up style scrolling: ctrl+b or PageUp
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_pty(ScrollDirection::Up, self.last_pty_size.0 as usize);
            }
            KeyCode::PageUp => {
                self.scroll_pty(ScrollDirection::Up, self.last_pty_size.0 as usize);
            }
            // Page-down style scrolling: ctrl+f or PageDown
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_pty(ScrollDirection::Down, self.last_pty_size.0 as usize);
            }
            KeyCode::PageDown => {
                self.scroll_pty(ScrollDirection::Down, self.last_pty_size.0 as usize);
            }
            _ => {}
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
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected_file + 1 < self.changed_files.len() {
                    self.selected_file += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_file > 0 {
                    self.selected_file -= 1;
                }
            }
            KeyCode::Enter => self.open_diff_for_selected_file()?,
            _ => {}
        }
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

        // ctrl+g exits interactive mode (like classic terminal escape).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            self.input_target = InputTarget::None;
            self.set_info("Exited interactive mode.");
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
        if let PromptState::Command {
            input,
            selected,
            searching,
        } = &mut self.prompt
        {
            match key.code {
                KeyCode::Esc => {
                    if *searching {
                        *searching = false;
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                KeyCode::Char('/') if !*searching => {
                    *searching = true;
                }
                KeyCode::Char('j') | KeyCode::Down if !*searching => {
                    let count = filtered_commands(input).len();
                    if *selected + 1 < count {
                        *selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up if !*searching => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Down if *searching => {
                    let count = filtered_commands(input).len();
                    if *selected + 1 < count {
                        *selected += 1;
                    }
                }
                KeyCode::Up if *searching => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                    *selected = 0;
                }
                KeyCode::Tab => {
                    if let Some(command) = filtered_commands(input).get(*selected) {
                        *input = command.name.to_string();
                        *selected = 0;
                    }
                }
                KeyCode::Enter => {
                    if *searching {
                        *searching = false;
                    } else {
                        let command = if let Some(command) = filtered_commands(input).get(*selected)
                        {
                            command.name.to_string()
                        } else {
                            input.trim().to_string()
                        };
                        self.prompt = PromptState::None;
                        if let Err(e) = self.execute_command(command) {
                            self.set_error(format!("{e:#}"));
                        }
                    }
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        input.push(c);
                        *selected = 0;
                    }
                }
                _ => {}
            }
            return Ok(false);
        }

        if let PromptState::BrowseProjects {
            current_dir,
            entries,
            loading,
            selected,
            filter,
            searching,
            editing_path,
            path_input,
            tab_completions,
            tab_index,
        } = &mut self.prompt
        {
            let mut browse_to: Option<PathBuf> = None;

            if *editing_path {
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
                            // Build completions from current input
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
                                        let name = e.file_name().to_string_lossy().to_lowercase();
                                        !name.starts_with('.') && name.starts_with(&prefix_lower)
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
                        } else {
                            // Cycle through existing completions
                            if key.code == KeyCode::BackTab {
                                if *tab_index == 0 {
                                    *tab_index = tab_completions.len().saturating_sub(1);
                                } else {
                                    *tab_index -= 1;
                                }
                            } else {
                                *tab_index = (*tab_index + 1) % tab_completions.len();
                            }
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
                            error_msg = Some(format!("{} is not a directory.", path_input.trim()));
                        }
                        *editing_path = false;
                        path_input.clear();
                        tab_completions.clear();
                        *tab_index = 0;
                    }
                    KeyCode::Char(c) => {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) {
                            path_input.push(c);
                            tab_completions.clear();
                            *tab_index = 0;
                        }
                    }
                    _ => {}
                }
                if let Some(msg) = error_msg {
                    self.set_error(msg);
                }
                if let Some(dir) = browse_to {
                    self.spawn_browser_entries(&dir);
                }
                return Ok(false);
            }

            let filtered_len = if filter.is_empty() {
                entries.len()
            } else {
                let needle = filter.to_lowercase();
                entries
                    .iter()
                    .filter(|e| e.label.to_lowercase().contains(&needle))
                    .count()
            };
            match key.code {
                KeyCode::Esc => {
                    if *searching {
                        *searching = false;
                    } else if !filter.is_empty() {
                        filter.clear();
                        *selected = 0;
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                KeyCode::Char('/') if !*searching => {
                    *searching = true;
                }
                KeyCode::Char('j') | KeyCode::Down if !*searching => {
                    if *selected + 1 < filtered_len {
                        *selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up if !*searching => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Down if *searching => {
                    if *selected + 1 < filtered_len {
                        *selected += 1;
                    }
                }
                KeyCode::Up if *searching => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Backspace if *searching => {
                    filter.pop();
                    *selected = 0;
                }
                KeyCode::Char('g') if !*searching => {
                    *editing_path = true;
                    let mut p = current_dir.to_string_lossy().to_string();
                    if !p.ends_with('/') {
                        p.push('/');
                    }
                    *path_input = p;
                }
                KeyCode::Enter if *searching => {
                    *searching = false;
                }
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if !*searching => {
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
                        if entry.is_git_repo {
                            let path = entry.path.to_string_lossy().to_string();
                            self.prompt = PromptState::None;
                            if let Err(e) = self.add_project(path, String::new()) {
                                self.set_error(format!("{e:#}"));
                            }
                        } else {
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
                KeyCode::Char(c) if *searching => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        filter.push(c);
                        *selected = 0;
                    }
                }
                _ => {}
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
            match key.code {
                KeyCode::Esc => self.prompt = PromptState::None,
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::Char('h')
                | KeyCode::Char('l') => {
                    *confirm_selected = !*confirm_selected;
                }
                KeyCode::Enter => {
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
            match key.code {
                KeyCode::Esc => self.prompt = PromptState::None,
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::Char('h')
                | KeyCode::Char('l') => {
                    *confirm_selected = !*confirm_selected;
                }
                KeyCode::Enter => {
                    if *confirm_selected {
                        return Ok(true);
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                _ => {}
            }
        }

        Ok(false)
    }

    fn handle_resize_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.left_width_pct = self.left_width_pct.saturating_sub(2).max(14)
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.left_width_pct = self.left_width_pct.saturating_add(2).min(38)
            }
            _ => {}
        }
    }

    fn persist_pane_widths(&mut self) {
        if self.config.ui.left_width_pct != self.left_width_pct
            || self.config.ui.right_width_pct != self.right_width_pct
        {
            self.config.ui.left_width_pct = self.left_width_pct;
            self.config.ui.right_width_pct = self.right_width_pct;
            let _ = save_config(&self.paths.config_path, &self.config);
        }
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(_) => {
                self.set_info(
                    "Mouse support is available for wheel navigation; resize has a keyboard fallback via Ctrl-w.",
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
                FocusPane::Files => {
                    if self.selected_file + 1 < self.changed_files.len() {
                        self.selected_file += 1;
                    }
                }
                _ => {}
            },
            MouseEventKind::ScrollUp => match self.focus {
                FocusPane::Left => {
                    if self.selected_left > 0 {
                        self.selected_left -= 1;
                        self.reload_changed_files();
                    }
                }
                FocusPane::Files => {
                    if self.selected_file > 0 {
                        self.selected_file -= 1;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}
