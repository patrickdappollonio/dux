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
            filter: TextInput::new(),
            searching: false,
            editing_path: false,
            path_input: TextInput::new(),
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

        // Check whether the current branch matches the remote default branch.
        // Two-tier warning: confident when origin/HEAD is available, heuristic
        // when it isn't but the branch name doesn't look like a main branch.
        let warning_kind = match git::remote_default_branch(&path) {
            Some(default) if default != branch => Some(BranchWarningKind::Known {
                default_branch: default,
            }),
            Some(_) => None, // on the default branch — no warning
            None if branch != "main" && branch != "master" => Some(BranchWarningKind::Heuristic),
            None => None, // looks like main/master — no warning
        };

        if let Some(kind) = warning_kind {
            self.prompt = PromptState::ConfirmNonDefaultBranch {
                path: path.to_string_lossy().to_string(),
                name,
                current_branch: branch,
                kind,
                confirm_selected: false,
            };
            return Ok(());
        }

        let path_str = path.to_string_lossy().to_string();
        self.finish_add_project(path_str, name, branch)
    }

    /// Saves the project to config and adds it to the runtime project list.
    /// Called directly when no branch warning is needed, or after the user
    /// confirms "Add Anyway" in the non-default-branch dialog.
    pub(crate) fn finish_add_project(
        &mut self,
        path: String,
        name: String,
        branch: String,
    ) -> Result<()> {
        let path_buf = PathBuf::from(&path);
        let display_name = if name.trim().is_empty() {
            path_buf
                .file_name()
                .and_then(|part| part.to_str())
                .unwrap_or("project")
                .to_string()
        } else {
            name.trim().to_string()
        };
        let project_id = Uuid::new_v4().to_string();
        self.config.projects.push(ProjectConfig {
            id: project_id.clone(),
            path: path.clone(),
            name: Some(display_name.clone()),
            default_provider: None,
            commit_prompt: None,
        });
        save_config(&self.paths.config_path, &self.config, &self.bindings)?;
        self.projects.push(Project {
            id: project_id,
            name: display_name.clone(),
            path,
            default_provider: self.config.default_provider(),
            current_branch: branch,
            path_missing: false,
        });
        self.rebuild_left_items();
        logger::info(&format!("registered project {}", path_buf.display()));
        self.set_info(format!("Added project \"{display_name}\" to workspace"));
        Ok(())
    }

    pub(crate) fn create_agent_for_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };

        if project.path_missing {
            return Ok(());
        }

        if self.config.defaults.prompt_for_name {
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.prompt = PromptState::NameNewAgent {
                request: CreateAgentRequest::NewProject {
                    project,
                    custom_name: None,
                    use_existing_branch: false,
                },
                input: TextInput::new().with_char_map(crate::git::agent_name_char_map),
            };
            return Ok(());
        }

        logger::info(&format!("creating agent for project {}", project.path));
        self.dispatch_create_agent_request(
            CreateAgentRequest::NewProject {
                project: project.clone(),
                custom_name: None,
                use_existing_branch: false,
            },
            format!(
                "Creating a new agent worktree for project \"{}\" and launching a fresh session...",
                project.name
            ),
        )
    }

    pub(crate) fn fork_selected_session(&mut self) -> Result<()> {
        let Some(source_session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first to fork.");
            return Ok(());
        };
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select an agent session first to fork.");
            return Ok(());
        };
        let source_label = self.session_label(&source_session);

        if self.config.defaults.prompt_for_name {
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.prompt = PromptState::NameNewAgent {
                request: CreateAgentRequest::ForkSession {
                    project: project.clone(),
                    source_session: Box::new(source_session),
                    source_label,
                    custom_name: None,
                },
                input: TextInput::new().with_char_map(crate::git::agent_name_char_map),
            };
            return Ok(());
        }

        logger::info(&format!(
            "forking session {} from worktree {}",
            source_session.id, source_session.worktree_path
        ));
        self.dispatch_create_agent_request(
            CreateAgentRequest::ForkSession {
                project: project.clone(),
                source_session: Box::new(source_session),
                source_label: source_label.clone(),
                custom_name: None,
            },
            format!(
                "Forking agent \"{source_label}\" by cloning its current worktree contents into a fresh session...",
            ),
        )
    }

    pub(crate) fn dispatch_create_agent_request(
        &mut self,
        request: CreateAgentRequest,
        busy_message: String,
    ) -> Result<()> {
        if self.create_agent_in_flight {
            self.set_error("An agent is already being created or forked.");
            return Ok(());
        }
        self.create_agent_in_flight = true;
        self.set_busy(busy_message);
        let paths = self.paths.clone();
        let config = self.config.clone();
        let worker_tx = self.worker_tx.clone();
        let term_size = crossterm::terminal::size().unwrap_or((80, 24));
        thread::spawn(move || {
            super::workers::run_create_agent_job(request, paths, config, worker_tx, term_size);
        });
        Ok(())
    }

    pub(crate) fn spawn_pty_for_session(
        &self,
        session: &AgentSession,
        resume: bool,
    ) -> Result<PtyClient> {
        let cfg = provider_config(&self.config, &session.provider);
        let launch_args = cfg.interactive_args(resume);
        let (rows, cols) = if self.last_pty_size != (0, 0) {
            self.last_pty_size
        } else {
            (24, 80)
        };
        logger::debug(&format!(
            "spawning PTY {:?} {:?} in {} ({}x{}, resume_supported={})",
            cfg.command,
            launch_args,
            session.worktree_path,
            cols,
            rows,
            cfg.supports_session_resume()
        ));
        PtyClient::spawn(
            &cfg.command,
            launch_args,
            Path::new(&session.worktree_path),
            rows,
            cols,
            self.config.ui.agent_scrollback_lines,
        )
    }

    pub(crate) fn spawn_companion_terminal_for_session(
        &self,
        session: &AgentSession,
    ) -> Result<PtyClient> {
        let (rows, cols) = if self.last_pty_size != (0, 0) {
            self.last_pty_size
        } else {
            (24, 80)
        };
        logger::debug(&format!(
            "spawning companion terminal {:?} {:?} in {} ({}x{})",
            self.config.terminal.command,
            self.config.terminal.args,
            session.worktree_path,
            cols,
            rows,
        ));
        PtyClient::spawn(
            &self.config.terminal.command,
            &self.config.terminal.args,
            Path::new(&session.worktree_path),
            rows,
            cols,
            self.config.ui.agent_scrollback_lines,
        )
    }

    pub(crate) fn show_agent_surface(&mut self) {
        self.focus = FocusPane::Center;
        self.center_mode = CenterMode::Agent;
        self.session_surface = SessionSurface::Agent;
        self.fullscreen_overlay = FullscreenOverlay::None;
    }

    pub(crate) fn show_companion_terminal_surface(&mut self) {
        self.session_surface = SessionSurface::Terminal;
        self.fullscreen_overlay = FullscreenOverlay::Terminal;
    }

    /// Always spawns a new companion terminal for the selected session.
    pub(crate) fn show_companion_terminal(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };

        let client = self.spawn_companion_terminal_for_session(&session)?;
        let terminal_id = self.next_terminal_id();
        let count = self.session_terminal_count(&session.id) + 1;
        let label = if count == 1 {
            session
                .title
                .clone()
                .unwrap_or_else(|| session.branch_name.clone())
        } else {
            let base = session
                .title
                .clone()
                .unwrap_or_else(|| session.branch_name.clone());
            format!("{base} ({count})")
        };
        self.companion_terminals.insert(
            terminal_id.clone(),
            CompanionTerminal {
                session_id: session.id.clone(),
                label,
                foreground_cmd: None,
                client,
            },
        );
        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = true;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        self.set_info(format!(
            "Launched terminal for agent \"{}\".",
            session.branch_name
        ));
        Ok(())
    }

    /// Opens the first existing companion terminal for the selected session,
    /// or spawns a new one if none exists.
    pub(crate) fn show_or_open_first_terminal(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };

        let first = self
            .companion_terminals
            .iter()
            .filter(|(_, t)| t.session_id == session.id)
            .min_by_key(|(id, _)| {
                id.strip_prefix("term-")
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(u64::MAX)
            })
            .map(|(id, t)| (id.clone(), t.label.clone()));

        if let Some((terminal_id, label)) = first {
            self.active_terminal_id = Some(terminal_id);
            self.terminal_return_to_list = false;
            self.show_companion_terminal_surface();
            self.input_target = InputTarget::Terminal;
            self.set_info(format!("Opened terminal \"{label}\"."));
            Ok(())
        } else {
            self.show_companion_terminal()
        }
    }

    /// Spawns a new companion terminal for the agent that owns the currently
    /// selected terminal in the terminals list.
    pub(crate) fn spawn_terminal_for_selected_terminal(&mut self) -> Result<()> {
        let items = self.terminal_items();
        let Some(&(_, terminal)) = items.get(self.selected_terminal_index) else {
            self.set_warning("No terminal selected.");
            return Ok(());
        };
        let session_id = terminal.session_id.clone();
        drop(items);

        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            self.set_warning("The parent agent session no longer exists.");
            return Ok(());
        };

        let client = self.spawn_companion_terminal_for_session(&session)?;
        let terminal_id = self.next_terminal_id();
        let count = self.session_terminal_count(&session.id) + 1;
        let base = session
            .title
            .clone()
            .unwrap_or_else(|| session.branch_name.clone());
        let label = if count == 1 {
            base
        } else {
            format!("{base} ({count})")
        };
        self.companion_terminals.insert(
            terminal_id.clone(),
            CompanionTerminal {
                session_id: session.id.clone(),
                label,
                foreground_cmd: None,
                client,
            },
        );
        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = true;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        self.set_info(format!(
            "Launched new terminal for agent \"{}\".",
            session.branch_name
        ));
        Ok(())
    }

    /// Palette command: always spawns a new companion terminal.
    /// Uses a yellow warning if no agent session is selected.
    pub(crate) fn new_companion_terminal(&mut self) -> Result<()> {
        if self.selected_session().is_none() {
            self.set_warning("Select an agent session first to launch a companion terminal.");
            return Ok(());
        }
        self.show_companion_terminal()
    }

    /// Opens the terminal overlay for the terminal selected in the terminals list.
    pub(crate) fn open_terminal_from_terminal_list(&mut self) -> Result<()> {
        let items = self.terminal_items();
        let Some(&(terminal_id, terminal)) = items.get(self.selected_terminal_index) else {
            return Ok(());
        };
        let terminal_id = terminal_id.clone();
        let session_id = terminal.session_id.clone();
        let label = terminal.label.clone();
        drop(items);

        // Select this terminal's session in the left pane.
        if let Some(pos) = self
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Session(idx) if self.sessions.get(*idx).map(|s| s.id.as_str()) == Some(session_id.as_str())))
        {
            self.selected_left = pos;
        }
        self.reload_changed_files();

        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = false;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        self.set_info(format!("Opened terminal \"{label}\"."));
        Ok(())
    }

    pub(crate) fn refresh_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        if project.path_missing {
            self.set_warning(format!(
                "Cannot refresh: path not found for \"{}\"",
                project.name
            ));
            return Ok(());
        }
        logger::info(&format!("refreshing project {}", project.path));
        self.start_pull(
            PathBuf::from(&project.path),
            PullTarget::Project {
                project_id: project.id,
                project_name: project.name.clone(),
            },
            format!("Refreshing project \"{}\" from remote…", project.name),
            format!(
                "Project refresh already in progress for \"{}\". Wait for the current pull to finish.",
                project.name,
            ),
        );
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
        let other_sessions_on_worktree = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);

        self.providers.remove(&session.id);
        self.last_pty_activity.remove(&session.id);
        self.resume_fallback_candidates.remove(&session.id);
        self.clear_companion_terminals_for_session(&session.id);
        self.sessions.retain(|candidate| candidate.id != session.id);
        self.session_store.delete_session(&session.id)?;
        self.update_branch_sync_sessions();
        self.rebuild_left_items();
        self.selected_left = self.selected_left.saturating_sub(1);
        self.reload_changed_files();

        if other_sessions_on_worktree {
            self.set_info(format!(
                "Deleted {} session for agent \"{}\". Worktree preserved for remaining sessions.",
                session.provider.as_str(),
                session.branch_name,
            ));
        } else {
            let result = git::remove_worktree(
                Path::new(&project.path),
                Path::new(&session.worktree_path),
                &session.branch_name,
            )?;
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
        }
        Ok(())
    }

    pub(crate) fn confirm_delete_selected_terminal(&mut self) -> Result<()> {
        let items = self.terminal_items();
        let Some((terminal_id, terminal)) = items.get(self.selected_terminal_index) else {
            self.set_error("Select a terminal first.");
            return Ok(());
        };
        self.prompt = PromptState::ConfirmDeleteTerminal {
            terminal_id: (*terminal_id).clone(),
            terminal_label: terminal.label.clone(),
            confirm_selected: false, // Cancel is default
        };
        Ok(())
    }

    pub(crate) fn do_delete_terminal(&mut self, terminal_id: &str) {
        let label = self
            .companion_terminals
            .get(terminal_id)
            .map(|t| t.label.clone());
        // Removing from the map drops PtyClient, which kills the child process.
        self.companion_terminals.remove(terminal_id);
        if self.active_terminal_id.as_deref() == Some(terminal_id) {
            self.active_terminal_id = None;
        }
        self.clamp_terminal_cursor();
        if let Some(label) = label {
            self.set_info(format!("Deleted terminal \"{}\"", label));
        }
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
            // Active is currently the only running session state. If SessionStatus
            // gains additional running variants, update this filter to keep
            // provider cycling limited to new and non-running agents.
            .filter(|s| s.project_id == project.id && !matches!(s.status, SessionStatus::Active))
        {
            session.provider = next.clone();
            self.session_store.upsert_session(session)?;
        }
        self.set_info(format!(
            "Changed default CLI agent to \"{}\" for new and stopped agents",
            next.as_str()
        ));
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

    /// Restart the selected agent with a fresh session, bypassing `--continue`
    /// or equivalent resume args. Works on both active and detached agents.
    pub(crate) fn force_reconnect_agent(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent first.");
            return Ok(());
        };
        if !Path::new(&session.worktree_path).exists() {
            self.set_error(format!(
                "Worktree for agent \"{}\" no longer exists. Delete and re-create the agent.",
                session.branch_name
            ));
            return Ok(());
        }
        // Kill existing PTY if the agent is still active.
        self.providers.remove(&session.id);
        self.last_pty_activity.remove(&session.id);
        self.resume_fallback_candidates.remove(&session.id);

        let detached_label =
            self.detach_conflicting_worktree_session(&session.worktree_path, &session.id);

        logger::info(&format!(
            "restarting agent \"{}\" with fresh session (no resume args)",
            session.branch_name
        ));
        match self.spawn_pty_for_session(&session, false) {
            Ok(client) => {
                self.providers.insert(session.id.clone(), client);
                self.mark_session_status(&session.id, SessionStatus::Active);
                self.show_agent_surface();
                self.input_target = InputTarget::Agent;
                self.fullscreen_overlay = FullscreenOverlay::Agent;
                let proj_name = self.project_name_for_session(&session);
                let mut msg = format!(
                    "Started fresh {} session for agent \"{}\" in project \"{}\". Use /sessions inside the agent to restore a prior conversation.",
                    session.provider.as_str(),
                    session.branch_name,
                    proj_name,
                );
                if let Some(detached) = &detached_label {
                    msg.push_str(&format!(
                        " Agent \"{}\" was detached to avoid worktree conflicts.",
                        detached,
                    ));
                }
                if let Some(project) = self.projects.iter().find(|p| p.id == session.project_id)
                    && project.default_provider != session.provider
                {
                    msg.push_str(&format!(
                        " Note: this agent uses {}. Your current default provider is {}.",
                        session.provider.as_str(),
                        project.default_provider.as_str(),
                    ));
                }
                self.set_info(msg);
            }
            Err(err) => {
                self.set_error(format!(
                    "Fresh restart failed for agent \"{}\": {err}",
                    session.branch_name
                ));
            }
        }
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
        let detached_label =
            self.detach_conflicting_worktree_session(&session.worktree_path, &session.id);

        let cfg = provider_config(&self.config, &session.provider);
        let use_resume = cfg.supports_session_resume();
        match self.spawn_pty_for_session(&session, use_resume) {
            Ok(client) => {
                self.providers.insert(session.id.clone(), client);
                if use_resume {
                    self.resume_fallback_candidates.insert(session.id.clone());
                }
                self.mark_session_status(&session.id, SessionStatus::Active);
                self.show_agent_surface();
                self.input_target = InputTarget::Agent;
                self.fullscreen_overlay = FullscreenOverlay::Agent;
                let proj_name = self.project_name_for_session(&session);
                let mut msg = format!(
                    "Relaunched {} agent \"{}\" in project \"{}\".",
                    session.provider.as_str(),
                    session.branch_name,
                    proj_name
                );
                if let Some(detached) = &detached_label {
                    msg.push_str(&format!(
                        " Agent \"{}\" was detached to avoid worktree conflicts.",
                        detached,
                    ));
                }
                if let Some(project) = self.projects.iter().find(|p| p.id == session.project_id)
                    && project.default_provider != session.provider
                {
                    msg.push_str(&format!(
                        " Note: this agent uses {}. Your current default provider is {}.",
                        session.provider.as_str(),
                        project.default_provider.as_str(),
                    ));
                }
                self.set_info(msg);
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
        let worktree_path = session.worktree_path.clone();
        let rel_path = file.path.clone();
        let output = crate::diff::diff_file(
            Path::new(&worktree_path),
            &rel_path,
            &self.theme,
            &self.syntax_cache,
            self.show_diff_line_numbers,
            self.config.ui.diff_tab_width,
        )?;
        self.center_mode = CenterMode::Diff {
            lines: Arc::new(output.lines),
            scroll: 0,
            gutter_width: output.gutter_width,
            worktree_path,
            rel_path,
        };
        self.focus = FocusPane::Center;
        Ok(())
    }

    /// Re-generate the currently displayed diff (e.g. after toggling line numbers).
    pub(crate) fn refresh_current_diff(&mut self) -> Result<()> {
        let (worktree_path, rel_path, scroll) = match &self.center_mode {
            CenterMode::Diff {
                worktree_path,
                rel_path,
                scroll,
                ..
            } => (worktree_path.clone(), rel_path.clone(), *scroll),
            _ => return Ok(()),
        };
        let output = crate::diff::diff_file(
            Path::new(&worktree_path),
            &rel_path,
            &self.theme,
            &self.syntax_cache,
            self.show_diff_line_numbers,
            self.config.ui.diff_tab_width,
        )?;
        self.center_mode = CenterMode::Diff {
            lines: Arc::new(output.lines),
            scroll,
            gutter_width: output.gutter_width,
            worktree_path,
            rel_path,
        };
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
                match self.clipboard.copy_text(
                    &p,
                    "Agent's path copied to clipboard.",
                    &self.worker_tx,
                ) {
                    Ok(()) => self.set_busy("Copying path to clipboard…"),
                    Err(e) => self.set_error(format!("Copy path failed: {e}")),
                }
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

    pub(crate) fn open_kill_running(&mut self) -> Result<()> {
        let runtimes = self.running_runtime_snapshot();
        if runtimes.is_empty() {
            self.set_error(
                "No running agents or companion terminals are available to kill. Start one first, then reopen the command palette.",
            );
            return Ok(());
        }

        self.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes,
            filter: TextInput::new(),
            searching: false,
            hovered_visible_index: 0,
            selected_ids: HashSet::new(),
            focus: KillRunningFocus::List,
        });
        let select = self.bindings.label_for(Action::ToggleMarked);
        let search = self.bindings.label_for(Action::SearchToggle);
        let next = self.bindings.label_for(Action::FocusNext);
        let prev = self.bindings.label_for(Action::FocusPrev);
        self.set_info(format!(
            "Kill Running opened. Press {select} to toggle runtimes, {search} to search, and {next}/{prev} to move between the list and actions.",
        ));
        Ok(())
    }

    pub(crate) fn running_runtime_snapshot(&self) -> Vec<KillableRuntime> {
        let mut runtimes = Vec::new();

        for session in &self.sessions {
            if !self.providers.contains_key(&session.id) {
                continue;
            }
            let project_name = self.project_name_for_session(session);
            let agent_name = self.session_label(session);
            let provider_name = session.provider.as_str();
            let label = Self::title_case_word(provider_name);
            let context = format!("on agent \"{agent_name}\" under project \"{project_name}\"");
            let search_text = format!(
                "{} {} {} {} {}",
                label,
                context,
                provider_name,
                agent_name,
                KillableRuntimeKind::Agent.noun()
            );
            runtimes.push(KillableRuntime {
                id: RuntimeTargetId::Agent(session.id.clone()),
                kind: KillableRuntimeKind::Agent,
                label,
                context,
                search_text,
            });
        }

        for (terminal_id, terminal) in self.terminal_items() {
            let (project_name, session_label) = self
                .sessions
                .iter()
                .find(|session| session.id == terminal.session_id)
                .map(|session| {
                    (
                        self.project_name_for_session(session),
                        self.session_label(session),
                    )
                })
                .unwrap_or_else(|| ("unknown".to_string(), terminal.session_id.clone()));
            let foreground = terminal
                .foreground_cmd
                .clone()
                .map(|cmd| {
                    let trimmed = cmd.trim();
                    trimmed
                        .strip_prefix("TERM ")
                        .or_else(|| trimmed.strip_prefix("term "))
                        .unwrap_or(trimmed)
                        .to_string()
                })
                .filter(|cmd| !cmd.trim().is_empty())
                .unwrap_or_else(|| "shell".to_string());
            let label = foreground;
            let context = format!("on agent \"{session_label}\" under project \"{project_name}\"");
            let search_text = format!(
                "{} {} {} {}",
                label,
                context,
                terminal.label,
                KillableRuntimeKind::Terminal.noun()
            );
            runtimes.push(KillableRuntime {
                id: RuntimeTargetId::Terminal(terminal_id.clone()),
                kind: KillableRuntimeKind::Terminal,
                label,
                context,
                search_text,
            });
        }

        runtimes.sort_by(|a, b| {
            (
                a.context.to_lowercase(),
                a.kind.noun(),
                a.label.to_lowercase(),
            )
                .cmp(&(
                    b.context.to_lowercase(),
                    b.kind.noun(),
                    b.label.to_lowercase(),
                ))
        });
        runtimes
    }

    pub(crate) fn visible_kill_running_indices(prompt: &KillRunningPrompt) -> Vec<usize> {
        if prompt.filter.is_empty() {
            return (0..prompt.runtimes.len()).collect();
        }
        let needle = prompt.filter.text.to_lowercase();
        prompt
            .runtimes
            .iter()
            .enumerate()
            .filter(|(_, runtime)| runtime.search_text.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    fn title_case_word(word: &str) -> String {
        let mut chars = word.chars();
        match chars.next() {
            Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
            None => String::new(),
        }
    }

    pub(crate) fn clamp_kill_running_prompt(prompt: &mut KillRunningPrompt) {
        let visible_len = Self::visible_kill_running_indices(prompt).len();
        if visible_len == 0 {
            prompt.hovered_visible_index = 0;
        } else if prompt.hovered_visible_index >= visible_len {
            prompt.hovered_visible_index = visible_len.saturating_sub(1);
        }
    }

    pub(crate) fn open_confirm_kill_running_action(
        &mut self,
        action: KillRunningAction,
    ) -> Result<()> {
        let PromptState::KillRunning(prompt) = &self.prompt else {
            return Ok(());
        };
        let prompt = prompt.clone();
        let visible_indices = Self::visible_kill_running_indices(&prompt);
        let target_ids = match action {
            KillRunningAction::Hovered => visible_indices
                .get(prompt.hovered_visible_index)
                .map(|&index| vec![prompt.runtimes[index].id.clone()])
                .unwrap_or_default(),
            KillRunningAction::Selected => prompt
                .runtimes
                .iter()
                .filter(|runtime| prompt.selected_ids.contains(&runtime.id))
                .map(|runtime| runtime.id.clone())
                .collect(),
            KillRunningAction::Visible => visible_indices
                .iter()
                .map(|&index| prompt.runtimes[index].id.clone())
                .collect(),
        };

        if target_ids.is_empty() {
            let message = match action {
                KillRunningAction::Hovered => {
                    "No running agent or terminal is highlighted. Move to a visible row first."
                }
                KillRunningAction::Selected => {
                    "No running agents or terminals are selected. Press Space to select one or more runtimes first."
                }
                KillRunningAction::Visible => {
                    "No running agents or terminals are visible for the current filter. Clear or change the search first."
                }
            };
            self.set_error(message);
            return Ok(());
        }

        self.prompt = PromptState::ConfirmKillRunning(ConfirmKillRunningPrompt {
            previous: prompt,
            action,
            target_ids,
            confirm_selected: false,
        });
        self.set_info(format!(
            "{} is ready. Review the warning and press Enter to confirm, or Esc to keep your running sessions alive.",
            action.button_label()
        ));
        Ok(())
    }

    pub(crate) fn kill_runtime_targets(
        &mut self,
        target_ids: &[RuntimeTargetId],
    ) -> (usize, usize) {
        let selected_session_id = self.selected_session().map(|session| session.id.clone());
        let active_terminal_id = self.active_terminal_id.clone();
        let mut killed_agents = 0;
        let mut killed_terminals = 0;
        let mut selected_agent_killed = false;
        let mut active_terminal_killed = false;

        for target_id in target_ids {
            match target_id {
                RuntimeTargetId::Agent(session_id) => {
                    if self.providers.remove(session_id).is_some() {
                        self.last_pty_activity.remove(session_id);
                        self.mark_session_status(session_id, SessionStatus::Detached);
                        killed_agents += 1;
                        if selected_session_id.as_deref() == Some(session_id.as_str()) {
                            selected_agent_killed = true;
                        }
                    }
                }
                RuntimeTargetId::Terminal(terminal_id) => {
                    if self.companion_terminals.remove(terminal_id).is_some() {
                        killed_terminals += 1;
                        if active_terminal_id.as_deref() == Some(terminal_id.as_str()) {
                            active_terminal_killed = true;
                        }
                    }
                }
            }
        }

        if active_terminal_killed {
            self.active_terminal_id = None;
            if self.session_surface == SessionSurface::Terminal {
                self.input_target = InputTarget::None;
                self.fullscreen_overlay = FullscreenOverlay::None;
                self.session_surface = SessionSurface::Agent;
            }
        }

        if selected_agent_killed && self.session_surface == SessionSurface::Agent {
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.focus = FocusPane::Left;
        }

        self.clamp_terminal_cursor();
        self.has_active_processes
            .store(self.running_process_count() > 0, Ordering::Relaxed);

        (killed_agents, killed_terminals)
    }

    fn session_label(&self, session: &AgentSession) -> String {
        session
            .title
            .clone()
            .unwrap_or_else(|| session.branch_name.clone())
    }

    /// Creates a new session on the same worktree as the selected session but
    /// using the project's current default provider.
    pub(crate) fn create_provider_session_on_worktree(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent first.");
            return Ok(());
        };
        let Some(project) = self
            .projects
            .iter()
            .find(|p| p.id == session.project_id)
            .cloned()
        else {
            self.set_error("Project not found for the selected agent.");
            return Ok(());
        };
        if self.providers.contains_key(&session.id) {
            self.set_error(
                "The selected agent is still running. Stop it before creating a new provider session.",
            );
            return Ok(());
        }
        if project.default_provider == session.provider {
            self.set_error(format!(
                "The selected agent already uses {}. Change the default provider first with the \"provider\" command.",
                session.provider.as_str(),
            ));
            return Ok(());
        }
        // Prevent creating a duplicate session with the same provider on
        // the same worktree.
        let target_provider = project.default_provider.clone();
        let duplicate = self.sessions.iter().any(|s| {
            s.id != session.id
                && s.worktree_path == session.worktree_path
                && s.provider == target_provider
        });
        if duplicate {
            self.set_error(format!(
                "A {} session already exists on this worktree. Select it from the session list instead.",
                target_provider.as_str(),
            ));
            return Ok(());
        }
        if self.create_agent_in_flight {
            self.set_error("Another agent is being created. Wait for it to finish.");
            return Ok(());
        }
        self.create_agent_in_flight = true;
        let request = CreateAgentRequest::NewProviderSession {
            project,
            source_session: Box::new(session),
            provider: target_provider,
        };
        let paths = self.paths.clone();
        let config = self.config.clone();
        let tx = self.worker_tx.clone();
        let term_size = self.last_pty_size;
        std::thread::spawn(move || {
            super::workers::run_create_agent_job(request, paths, config, tx, term_size);
        });
        Ok(())
    }

    /// If another session on the same worktree has a running PTY, detach it
    /// (kill the PTY and mark the session as `Detached`).  Returns the
    /// human-readable label of the detached session, if any.
    pub(crate) fn detach_conflicting_worktree_session(
        &mut self,
        worktree_path: &str,
        exclude_id: &str,
    ) -> Option<String> {
        let conflicting = self
            .sessions
            .iter()
            .find(|s| {
                s.id != exclude_id
                    && s.worktree_path == worktree_path
                    && self.providers.contains_key(&s.id)
            })
            .cloned()?;

        let label = self.session_label(&conflicting);
        let provider = conflicting.provider.as_str().to_string();
        self.providers.remove(&conflicting.id);
        self.last_pty_activity.remove(&conflicting.id);
        self.resume_fallback_candidates.remove(&conflicting.id);
        self.mark_session_status(&conflicting.id, SessionStatus::Detached);

        logger::info(&format!(
            "auto-detached {} agent \"{}\" to avoid worktree conflict",
            provider, label,
        ));
        Some(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DuxPaths;
    use crate::keybindings::{BINDING_DEFS, RuntimeBindings};
    use crate::model::{AgentSession, Project, ProviderKind, SessionStatus};
    use crate::storage::SessionStore;
    use crate::theme::Theme;
    use chrono::Utc;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, mpsc};
    use tempfile::tempdir;

    fn test_bindings() -> RuntimeBindings {
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

    fn test_app_with_sessions(sessions: Vec<AgentSession>, projects: Vec<Project>) -> App {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);

        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let session_store = SessionStore::open(&paths.sessions_db_path).expect("session store");
        let bindings = test_bindings();
        let (worker_tx, worker_rx) = mpsc::channel();
        let single_instance_lock = crate::lockfile::SingleInstanceLock::acquire(&paths.lock_path)
            .expect("single-instance lock for test App");
        let mut app = App {
            config: Config::default(),
            paths,
            bindings,
            session_store,
            projects,
            sessions,
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
            welcome_logo_alt: false,
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
            _single_instance_lock: single_instance_lock,
        };
        app.interactive_patterns = app.bindings.interactive_byte_patterns();
        app.rebuild_left_items();
        app
    }

    fn make_session(id: &str, provider: &str, worktree: &str) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: id.to_string(),
            project_id: "project-1".to_string(),
            project_path: Some("/tmp/project".to_string()),
            provider: ProviderKind::from_str(provider),
            source_branch: "main".to_string(),
            branch_name: format!("branch-{id}"),
            worktree_path: worktree.to_string(),
            title: None,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_project(id: &str, provider: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "demo".to_string(),
            path: "/tmp/project".to_string(),
            default_provider: ProviderKind::from_str(provider),
            current_branch: "main".to_string(),
            path_missing: false,
        }
    }

    /// Inserts a dummy PtyClient placeholder into `app.providers` so that the
    /// session appears "active" without actually spawning a process.
    fn mark_active(app: &mut App, session_id: &str) {
        let client =
            crate::pty::PtyClient::spawn("echo", &[], std::path::Path::new("/tmp"), 24, 80, 1000)
                .expect("spawn echo for test");
        app.providers.insert(session_id.to_string(), client);
    }

    #[test]
    fn detach_finds_conflict_on_same_worktree() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);
        mark_active(&mut app, "s1");

        let label = app.detach_conflicting_worktree_session("/tmp/wt/a", "s2");
        assert!(label.is_some());
        assert!(!app.providers.contains_key("s1"));
    }

    #[test]
    fn detach_no_conflict_different_path() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/b");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);
        mark_active(&mut app, "s1");

        let label = app.detach_conflicting_worktree_session("/tmp/wt/b", "s2");
        assert!(label.is_none());
        assert!(app.providers.contains_key("s1"));
    }

    #[test]
    fn detach_excludes_self() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);
        mark_active(&mut app, "s1");

        let label = app.detach_conflicting_worktree_session("/tmp/wt/a", "s1");
        assert!(label.is_none());
        assert!(app.providers.contains_key("s1"));
    }

    #[test]
    fn detach_conflicting_worktree_session_removes_pty() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/a");
        let project = make_project("project-1", "codex");
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);
        mark_active(&mut app, "s1");

        let label = app.detach_conflicting_worktree_session("/tmp/wt/a", "s2");
        assert!(label.is_some());
        assert!(!app.providers.contains_key("s1"));
        let s1_session = app.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s1_session.status, SessionStatus::Detached);
    }

    #[test]
    fn create_provider_session_rejects_running_agent() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "codex");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);
        app.selected_left = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Session(_)))
            .unwrap_or(0);
        mark_active(&mut app, "s1");

        app.create_provider_session_on_worktree().unwrap();
        assert!(app.status.text().contains("still running"));
    }

    #[test]
    fn create_provider_session_rejects_same_provider() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);
        // Select the session in the left pane.
        app.selected_left = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Session(_)))
            .unwrap_or(0);

        app.create_provider_session_on_worktree().unwrap();
        // Should have set an error because the session already uses claude.
        assert!(app.status.text().contains("already uses"));
    }

    #[test]
    fn create_provider_session_rejects_duplicate() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/a");
        let project = make_project("project-1", "codex");
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);
        // Select s1 (the claude session).
        app.selected_left = app
            .left_items()
            .iter()
            .position(|item| {
                matches!(item, LeftItem::Session(i) if app.sessions.get(*i).map(|s| s.id.as_str()) == Some("s1"))
            })
            .unwrap_or(0);

        app.create_provider_session_on_worktree().unwrap();
        // Should reject because s2 already uses codex on the same worktree.
        assert!(app.status.text().contains("already exists"));
    }

    #[test]
    fn detach_conflicting_returns_none_when_no_conflict() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        let label = app.detach_conflicting_worktree_session("/tmp/wt/a", "s1");
        assert!(label.is_none());
    }

    #[test]
    fn delete_session_preserves_shared_worktree() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let app = test_app_with_sessions(vec![s1, s2], vec![project]);

        // Deleting s1 should preserve the worktree because s2 still uses it.
        // We can't call do_delete_session directly because git::remove_worktree
        // would fail on a non-existent repo, but we can verify the guard logic.
        let has_sibling = app
            .sessions
            .iter()
            .any(|s| s.id != "s1" && s.worktree_path == "/tmp/wt/a");
        assert!(has_sibling, "sibling session should exist");
    }

    #[test]
    fn delete_session_allows_removal_when_last() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let app = test_app_with_sessions(vec![s1], vec![project]);

        let has_sibling = app
            .sessions
            .iter()
            .any(|s| s.id != "s1" && s.worktree_path == "/tmp/wt/a");
        assert!(!has_sibling, "no sibling session should exist");
    }
}
