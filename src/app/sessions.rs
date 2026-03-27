use super::*;
use crate::editor;

impl App {
    pub(crate) fn open_project_browser(&mut self) -> Result<()> {
        let start_dir = self
            .config
            .defaults
            .start_directory
            .as_ref()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| {
                std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    })
            });
        self.prompt = PromptState::BrowseProjects {
            current_dir: start_dir.clone(),
            entries: Vec::new(),
            loading: true,
            selected: 0,
            filter: String::new(),
            searching: false,
            editing_path: false,
            path_input: String::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        };
        self.spawn_browser_entries(&start_dir);
        {
            let open = self.bindings.label_for(Action::OpenEntry);
            let add = self.bindings.label_for(Action::AddCurrentDir);
            let search = self.bindings.label_for(Action::SearchToggle);
            let goto = self.bindings.label_for(Action::GoToPath);
            self.set_info(format!(
                "Project browser: {open} opens folders, {add} adds current dir, {search} to search, {goto} to go to a path.",
            ));
        }
        Ok(())
    }

    pub(crate) fn add_project(&mut self, raw_path: String, name: String) -> Result<()> {
        let path = PathBuf::from(raw_path.trim())
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(raw_path.trim()));
        logger::info(&format!("attempting to add project {}", path.display()));
        if !path.exists() || !git::is_git_repo(&path) {
            logger::error(&format!("add project rejected for {}", path.display()));
            self.set_error(format!("\"{}\" is not a git repository.", path.display()));
            return Ok(());
        }
        if self
            .projects
            .iter()
            .any(|project| Path::new(&project.path) == path.as_path())
        {
            self.set_error(format!(
                "\"{}\" is already registered as a project.",
                path.display()
            ));
            return Ok(());
        }
        let branch = git::current_branch(&path)?;
        let display_name = if name.trim().is_empty() {
            path.file_name()
                .and_then(|part| part.to_str())
                .unwrap_or("project")
                .to_string()
        } else {
            name.trim().to_string()
        };
        let project_id = Uuid::new_v4().to_string();
        self.config.projects.push(ProjectConfig {
            id: project_id.clone(),
            path: path.to_string_lossy().to_string(),
            name: Some(display_name.clone()),
            default_provider: None,
            commit_prompt: None,
        });
        save_config(&self.paths.config_path, &self.config, &self.bindings)?;
        self.projects.push(Project {
            id: project_id,
            name: display_name.clone(),
            path: path.to_string_lossy().to_string(),
            default_provider: self.config.default_provider(),
            current_branch: branch,
        });
        self.rebuild_left_items();
        logger::info(&format!("registered project {}", path.display()));
        self.set_info(format!("Added project \"{display_name}\" to workspace"));
        Ok(())
    }

    pub(crate) fn create_agent_for_selected_project(&mut self) -> Result<()> {
        if self.create_agent_in_flight {
            self.set_error("An agent is already being created.");
            return Ok(());
        }
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        logger::info(&format!("creating agent for project {}", project.path));
        self.create_agent_in_flight = true;
        self.set_busy(format!(
            "Creating worktree for project \"{}\"...",
            project.name
        ));
        let paths = self.paths.clone();
        let config = self.config.clone();
        let worker_tx = self.worker_tx.clone();
        let term_size = crossterm::terminal::size().unwrap_or((80, 24));
        thread::spawn(move || {
            super::workers::run_create_agent_job(project, paths, config, worker_tx, term_size);
        });
        Ok(())
    }

    pub(crate) fn spawn_pty_for_session(&self, session: &AgentSession) -> Result<PtyClient> {
        let cfg = provider_config(&self.config, &session.provider);
        let (rows, cols) = if self.last_pty_size != (0, 0) {
            self.last_pty_size
        } else {
            (24, 80)
        };
        logger::debug(&format!(
            "spawning PTY {:?} {:?} in {} ({}x{})",
            cfg.command, cfg.args, session.worktree_path, cols, rows
        ));
        PtyClient::spawn(
            &cfg.command,
            &cfg.args,
            Path::new(&session.worktree_path),
            rows,
            cols,
            self.config.ui.agent_scrollback_lines,
        )
    }

    pub(crate) fn refresh_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        logger::info(&format!("refreshing project {}", project.path));
        let path = Path::new(&project.path);
        if git::is_dirty(path)? {
            self.set_error("Refresh blocked because the source checkout has uncommitted changes.");
            return Ok(());
        }
        let output = git::pull_current_branch(path)?;
        if let Some(existing) = self
            .projects
            .iter_mut()
            .find(|candidate| candidate.id == project.id)
        {
            existing.current_branch =
                git::current_branch(path).unwrap_or_else(|_| existing.current_branch.clone());
        }
        self.set_info(format!(
            "Refreshed project \"{}\": {}",
            project.name,
            output.trim()
        ));
        Ok(())
    }

    pub(crate) fn confirm_delete_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        self.prompt = PromptState::ConfirmDeleteAgent {
            session_id: session.id.clone(),
            branch_name: session.branch_name.clone(),
            confirm_selected: false, // Cancel is default
        };
        Ok(())
    }

    pub(crate) fn do_delete_session(&mut self, session_id: &str) -> Result<()> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            return Ok(());
        };
        logger::info(&format!(
            "deleting session {} at {}",
            session.id, session.worktree_path
        ));
        let Some(project) = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned()
        else {
            return Ok(());
        };
        let result = git::remove_worktree(
            Path::new(&project.path),
            Path::new(&session.worktree_path),
            &session.branch_name,
        )?;
        self.providers.remove(&session.id);
        self.sessions.retain(|candidate| candidate.id != session.id);
        self.session_store.delete_session(&session.id)?;
        self.rebuild_left_items();
        self.selected_left = self.selected_left.saturating_sub(1);
        self.reload_changed_files();
        if result.branch_already_deleted {
            self.set_info(format!(
                "Deleted agent (branch \"{}\" was already removed)",
                session.branch_name
            ));
        } else {
            self.set_info(format!(
                "Deleted {} agent from project \"{}\" with branch \"{}\"",
                session.provider.as_str(),
                project.name,
                session.branch_name
            ));
        }
        Ok(())
    }

    pub(crate) fn cycle_selected_project_provider(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        let provider_names: Vec<&String> = self.config.providers.commands.keys().collect();
        if provider_names.is_empty() {
            self.set_error("No providers configured.");
            return Ok(());
        }
        let current = project.default_provider.as_str();
        let current_idx = provider_names
            .iter()
            .position(|n| n.as_str() == current)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % provider_names.len();
        let next = ProviderKind::new(provider_names[next_idx].clone());
        if let Some(existing) = self
            .projects
            .iter_mut()
            .find(|candidate| candidate.id == project.id)
        {
            existing.default_provider = next.clone();
        }
        if let Some(project_config) = self
            .config
            .projects
            .iter_mut()
            .find(|candidate| Path::new(&candidate.path) == Path::new(&project.path))
        {
            project_config.default_provider = Some(next.as_str().to_string());
        }
        save_config(&self.paths.config_path, &self.config, &self.bindings)?;
        for session in self
            .sessions
            .iter_mut()
            .filter(|s| s.project_id == project.id)
        {
            session.provider = next.clone();
            self.session_store.upsert_session(session)?;
        }
        self.set_info(format!("Changed CLI agent to \"{}\"", next.as_str()));
        Ok(())
    }

    pub(crate) fn remove_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        let has_sessions = self.sessions.iter().any(|s| s.project_id == project.id);
        if has_sessions {
            self.set_error("Delete all agents in this project first.");
            return Ok(());
        }
        self.projects.retain(|p| p.id != project.id);
        self.config
            .projects
            .retain(|p| Path::new(&p.path) != Path::new(&project.path));
        save_config(&self.paths.config_path, &self.config, &self.bindings)?;
        self.rebuild_left_items();
        self.selected_left = self.selected_left.saturating_sub(1);
        self.set_info(format!("Removed project \"{}\" from app", project.name));
        Ok(())
    }

    pub(crate) fn delete_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        logger::info(&format!("deleting project {}", project.path));
        let session_ids = self
            .sessions
            .iter()
            .filter(|session| session.project_id == project.id)
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        for session_id in session_ids {
            if let Some(index) = self
                .sessions
                .iter()
                .position(|session| session.id == session_id)
            {
                self.selected_left = self
                    .left_items()
                    .iter()
                    .position(
                        |item| matches!(item, LeftItem::Session(session_index) if *session_index == index),
                    )
                    .unwrap_or(self.selected_left);
                self.do_delete_session(&session_id)?;
            }
        }
        self.projects.retain(|candidate| candidate.id != project.id);
        self.config
            .projects
            .retain(|candidate| Path::new(&candidate.path) != Path::new(&project.path));
        save_config(&self.paths.config_path, &self.config, &self.bindings)?;
        self.rebuild_left_items();
        self.selected_left = self.selected_left.saturating_sub(1);
        self.reload_changed_files();
        self.set_info(format!(
            "Deleted project \"{}\" and all its agents",
            project.name
        ));
        Ok(())
    }

    pub(crate) fn reconnect_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select a stopped agent first to reconnect.");
            return Ok(());
        };
        logger::info(&format!("reconnecting session {}", session.id));
        if self.providers.contains_key(&session.id) {
            self.set_info(format!(
                "Agent \"{}\" is already connected.",
                session.branch_name
            ));
            return Ok(());
        }
        if !Path::new(&session.worktree_path).exists() {
            self.set_error(format!(
                "Worktree for agent \"{}\" no longer exists. Delete and re-create the agent.",
                session.branch_name
            ));
            return Ok(());
        }
        match self.spawn_pty_for_session(&session) {
            Ok(client) => {
                self.providers.insert(session.id.clone(), client);
                self.mark_session_status(&session.id, SessionStatus::Active);
                self.focus = FocusPane::Center;
                self.center_mode = CenterMode::Agent;
                self.input_target = InputTarget::Agent;
                self.fullscreen_agent = true;
                let proj_name = self.project_name_for_session(&session);
                self.set_info(format!(
                    "Relaunched {} agent \"{}\" in project \"{}\"",
                    session.provider.as_str(),
                    session.branch_name,
                    proj_name
                ));
            }
            Err(err) => {
                self.set_error(format!(
                    "Reconnect failed for agent \"{}\": {err}",
                    session.branch_name
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn open_diff_for_selected_file(&mut self) -> Result<()> {
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let Some(file) = self.selected_changed_file() else {
            return Ok(());
        };
        let output =
            crate::diff::diff_file(Path::new(&session.worktree_path), &file.path, &self.theme)?;
        self.center_mode = CenterMode::Diff {
            lines: output.lines,
            scroll: 0,
        };
        self.focus = FocusPane::Center;
        Ok(())
    }

    pub(crate) fn copy_selected_path(&mut self) -> Result<()> {
        let path = match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => {
                self.sessions.get(*index).map(|s| s.worktree_path.clone())
            }
            Some(LeftItem::Project(index)) => self.projects.get(*index).map(|p| p.path.clone()),
            None => None,
        };
        match path {
            Some(p) => {
                let mut clipboard = arboard::Clipboard::new()
                    .map_err(|e| anyhow::anyhow!("Failed to access clipboard: {e}"))?;
                clipboard
                    .set_text(&p)
                    .map_err(|e| anyhow::anyhow!("Failed to copy to clipboard: {e}"))?;
                self.set_info(format!("Copied path to clipboard: \"{p}\""));
                Ok(())
            }
            None => {
                self.set_error("No project or agent selected. Select one from the sidebar first.");
                Ok(())
            }
        }
    }

    pub(crate) fn open_selected_worktree_in_default_editor(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };
        let editors = editor::detect_installed_editors();
        let Some(selected_editor) = editor::preferred_editor(&editors, &self.config.editor.default)
        else {
            self.set_error(
                "No supported editor CLI found on PATH. Install cursor, code, zed, or antigravity.",
            );
            return Ok(());
        };

        let session_label = self.session_label(&session);
        let configured_default = self.config.editor.default.trim().to_string();
        self.open_worktree_in_editor(&session.worktree_path, &session_label, &selected_editor)?;

        if !configured_default.is_empty()
            && !editor::matches_configured_editor(&selected_editor, &configured_default)
        {
            self.set_info(format!(
                "Opened agent \"{session_label}\" in {} via {} (configured default \"{}\" was not found on PATH).",
                selected_editor.label, selected_editor.command, configured_default
            ));
        }

        Ok(())
    }

    pub(crate) fn open_worktree_editor_picker(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };
        let editors = editor::detect_installed_editors();
        if editors.is_empty() {
            self.set_error(
                "No supported editor CLI found on PATH. Install cursor, code, zed, or antigravity.",
            );
            return Ok(());
        }

        let selected = editor::preferred_editor(&editors, &self.config.editor.default)
            .and_then(|preferred| {
                editors
                    .iter()
                    .position(|editor| editor.command == preferred.command)
            })
            .unwrap_or(0);
        let session_label = self.session_label(&session);
        self.prompt = PromptState::PickEditor {
            session_label,
            worktree_path: session.worktree_path.clone(),
            editors,
            selected,
        };
        self.set_info("Choose an editor and press Enter to open the selected worktree.");
        Ok(())
    }

    pub(crate) fn open_worktree_in_editor(
        &mut self,
        worktree_path: &str,
        session_label: &str,
        editor_choice: &editor::DetectedEditor,
    ) -> Result<()> {
        editor::launch_editor(editor_choice, Path::new(worktree_path))?;
        self.set_info(format!(
            "Opened agent \"{session_label}\" in {} via {}.",
            editor_choice.label, editor_choice.command
        ));
        Ok(())
    }

    fn session_label(&self, session: &AgentSession) -> String {
        session
            .title
            .clone()
            .unwrap_or_else(|| session.branch_name.clone())
    }
}
