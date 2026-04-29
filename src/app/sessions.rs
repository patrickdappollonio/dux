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
            // Default the checkbox to on for the confident path so hitting
            // Enter resolves the warning in the way users typically want —
            // "switch to main, then add". The heuristic path ignores this
            // field (no checkbox is shown).
            let checkout_default = matches!(kind, BranchWarningKind::Known { .. });
            self.prompt = PromptState::ConfirmNonDefaultBranch {
                path: path.to_string_lossy().to_string(),
                name,
                current_branch: branch,
                kind,
                focus: ConfirmNonDefaultBranchFocus::Cancel,
                checkout_default,
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

        self.open_name_new_agent_prompt(CreateAgentRequest::NewProject {
            project,
            custom_name: None,
            use_existing_branch: false,
        })
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

        self.open_name_new_agent_prompt(CreateAgentRequest::ForkSession {
            project,
            source_session: Box::new(source_session),
            source_label,
            custom_name: None,
        })
    }

    fn open_name_new_agent_prompt(&mut self, request: CreateAgentRequest) -> Result<()> {
        let randomize_name = self.config.defaults.enable_randomized_pet_name_by_default;
        let mut input = TextInput::new().with_char_map(crate::git::agent_name_char_map);
        let mut randomized_name = None;
        if randomize_name {
            let name = crate::git::docker_style_name();
            input.set_text(name.clone());
            randomized_name = Some(name);
        }

        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::NameNewAgent {
            request,
            input,
            randomize_name,
            randomized_name,
            focus: NameNewAgentFocus::Input,
        };
        Ok(())
    }

    /// Spawns a background worker that runs `git switch <target_branch>` in
    /// the source repo before registering the project. On success, the
    /// `WorkerEvent::AddProjectCheckoutCompleted` handler calls
    /// `finish_add_project`; on failure it surfaces the git error.
    pub(crate) fn dispatch_add_project_checkout(
        &mut self,
        path: String,
        name: String,
        target_branch: String,
    ) {
        self.set_busy(format!(
            "Checking out \"{target_branch}\" in {path} before adding the project..."
        ));
        let worker_tx = self.worker_tx.clone();
        thread::spawn(move || {
            super::workers::run_add_project_checkout_job(path, name, target_branch, worker_tx);
        });
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

    pub(crate) fn should_resume_session(&self, session: &AgentSession) -> bool {
        let cfg = provider_config(&self.config, &session.provider);
        cfg.supports_session_resume() && session.has_started_provider(&session.provider)
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
        let worktree_shared = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);
        self.prompt = PromptState::ConfirmDeleteAgent {
            session_id: session.id.clone(),
            branch_name: session.branch_name.clone(),
            focus: DeleteAgentFocus::Cancel, // Cancel is the safe default
            delete_worktree: false,          // Opt-in destructive action
            worktree_shared,
        };
        Ok(())
    }

    /// Delete the agent session identified by `session_id`, blocking the
    /// calling thread for any git work. Used by bulk flows like project
    /// deletion, where we must complete all removals before the parent
    /// operation proceeds. User-initiated single-agent deletes go through
    /// [`begin_delete_session`] so git work runs off the UI thread.
    ///
    /// When `delete_worktree` is true AND no other sessions share the worktree,
    /// the git worktree and branch are removed first. If the git removal fails,
    /// the session record is preserved so the caller can retry without losing
    /// the agent. When `delete_worktree` is false, the worktree and branch
    /// are always preserved.
    pub(crate) fn do_delete_session(
        &mut self,
        session_id: &str,
        delete_worktree: bool,
    ) -> Result<()> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            return Ok(());
        };
        logger::info(&format!(
            "deleting session {} at {} (delete_worktree={}, sync)",
            session.id, session.worktree_path, delete_worktree
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

        let should_remove_worktree = delete_worktree && !other_sessions_on_worktree;

        // Attempt git operations FIRST so a failure leaves the agent intact.
        // Worktrees are user data — if we can't remove the worktree cleanly, we
        // must not leave the user with a deleted agent record and an orphaned
        // worktree on disk.
        //
        // Callers must ensure no async worker is already removing this
        // worktree (`pending_deletions` should not contain this session).
        // `delete_selected_project` checks this at entry; `begin_delete_session`
        // uses a separate async path entirely. If a caller violates this
        // contract two concurrent git calls will race, so we assert in debug.
        debug_assert!(
            !self.pending_deletions.contains(session_id),
            "do_delete_session called while an async delete worker is in-flight for {}",
            session_id,
        );
        let remove_outcome = if should_remove_worktree {
            let result = git::remove_worktree(
                Path::new(&project.path),
                Path::new(&session.worktree_path),
                &session.branch_name,
            )?;
            Some(result.branch_already_deleted)
        } else {
            None
        };

        self.finish_delete_session(session_id, delete_worktree, remove_outcome, true)?;
        Ok(())
    }

    /// Kick off deletion of `session_id` from the user-facing modal.
    ///
    /// When the git worktree needs to be removed, the `git worktree remove`
    /// call is dispatched to a background thread and the session record is
    /// left in place until the worker reports success via
    /// [`WorkerEvent::WorktreeRemoveCompleted`]. This keeps the UI responsive
    /// even when git stalls (slow disk, held lock, large worktree). When no
    /// git work is required the session is cleaned up synchronously — that
    /// path only touches in-memory state and SQLite, which is effectively
    /// instantaneous.
    pub(crate) fn begin_delete_session(&mut self, session_id: &str, delete_worktree: bool) {
        // Reject duplicate delete requests for the same session. The first
        // worker will clean up when it finishes; spawning a second one would
        // just race and confuse the status line.
        if self.pending_deletions.contains(session_id) {
            self.set_error("Deletion already in progress for this agent. Wait for it to finish.");
            return;
        }

        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            return;
        };
        let Some(project) = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned()
        else {
            return;
        };
        let other_sessions_on_worktree = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);
        let should_remove_worktree = delete_worktree && !other_sessions_on_worktree;

        if should_remove_worktree {
            logger::info(&format!(
                "deleting session {} at {} (delete_worktree=true, async)",
                session.id, session.worktree_path
            ));
            // Mark in-flight BEFORE spawning, so a fast follow-up action from
            // the same event loop tick can see the guard. The worker event
            // handler clears the entry on completion (Ok or Err).
            self.pending_deletions.insert(session.id.clone());
            let sid = session.id.clone();
            let project_path = project.path.clone();
            let worktree_path = session.worktree_path.clone();
            let branch_name = session.branch_name.clone();
            let tx = self.worker_tx.clone();
            std::thread::spawn(move || {
                let result = git::remove_worktree(
                    Path::new(&project_path),
                    Path::new(&worktree_path),
                    &branch_name,
                )
                .map(|r| r.branch_already_deleted)
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(WorkerEvent::WorktreeRemoveCompleted {
                    session_id: sid,
                    result,
                });
            });
            let busy_msg = format!(
                "Removing worktree for agent \"{}\"\u{2026}",
                session.branch_name
            );
            self.set_busy(&busy_msg);
            self.deletion_busy_messages
                .insert(session.id.clone(), busy_msg);
        } else {
            logger::info(&format!(
                "deleting session {} at {} (delete_worktree={}, inline)",
                session.id, session.worktree_path, delete_worktree
            ));
            if let Err(e) = self.finish_delete_session(session_id, delete_worktree, None, true) {
                self.set_error(format!("{e:#}"));
            }
        }
    }

    /// Remove all local bookkeeping for a session whose git side has already
    /// been handled (or does not need handling). Idempotent — if the session
    /// is no longer present this is a no-op, which matters for the async path
    /// where the user may have deleted the project before the worker replies.
    ///
    /// `remove_outcome` is `Some(branch_already_deleted)` only on the branch
    /// where we actually removed the worktree; it drives the success message
    /// variant.
    /// `update_status` controls whether the method writes a success message
    /// to the status line. The async worker handler passes `false` when the
    /// status line has already been overwritten by an unrelated operation
    /// (push, pull, etc.) to avoid clobbering it. Synchronous callers and
    /// the handler's "our Busy is still showing" path pass `true`.
    pub(crate) fn finish_delete_session(
        &mut self,
        session_id: &str,
        delete_worktree: bool,
        remove_outcome: Option<bool>,
        update_status: bool,
    ) -> Result<()> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            return Ok(());
        };
        let project = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned();
        let other_sessions_on_worktree = self
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);

        // Persist the deletion FIRST so a DB failure leaves in-memory state
        // untouched and the session remains visible in the UI. If we cleared
        // in-memory state first and the DB call then failed, the session
        // would vanish from the UI but reappear on restart.
        self.session_store.delete_session(&session.id)?;

        self.providers.remove(&session.id);
        self.running_provider_pins.remove(&session.id);
        self.last_pty_activity.remove(&session.id);
        self.resume_fallback_candidates.remove(&session.id);
        self.clear_companion_terminals_for_session(&session.id);
        self.sessions.retain(|candidate| candidate.id != session.id);
        self.update_branch_sync_sessions();
        self.rebuild_left_items();
        self.selected_left = self.selected_left.saturating_sub(1);
        self.reload_changed_files();

        // Detect contract violation unconditionally, regardless of
        // update_status. Callers that pass delete_worktree=true with no
        // siblings must have already run git::remove_worktree and produced
        // Some(outcome). Return Err so the violation surfaces in callers
        // and tests rather than being silently treated as a success.
        // Note: session cleanup above already ran (DB + memory), so the
        // session is gone; the Err signals the broken invariant, not an
        // incomplete deletion.
        if !other_sessions_on_worktree && delete_worktree && remove_outcome.is_none() {
            return Err(anyhow::anyhow!(
                "Internal error: worktree deletion flagged but no removal result provided."
            ));
        }

        if update_status {
            match (other_sessions_on_worktree, delete_worktree, remove_outcome) {
                (true, true, _) => {
                    self.set_info(format!(
                        "Deleted {} agent \"{}\". Worktree preserved because other sessions still use it.",
                        session.provider.as_str(),
                        session.branch_name,
                    ));
                }
                (true, false, _) => {
                    self.set_info(format!(
                        "Deleted {} session for agent \"{}\". Worktree preserved for remaining sessions.",
                        session.provider.as_str(),
                        session.branch_name,
                    ));
                }
                (false, false, _) => {
                    self.set_info(format!(
                        "Deleted {} agent \"{}\". Worktree preserved at {}.",
                        session.provider.as_str(),
                        session.branch_name,
                        session.worktree_path,
                    ));
                }
                (false, true, Some(branch_already_deleted)) => {
                    if branch_already_deleted {
                        self.set_info(format!(
                            "Deleted agent (branch \"{}\" was already removed)",
                            session.branch_name
                        ));
                    } else {
                        let project_name = project
                            .as_ref()
                            .map(|p| p.name.as_str())
                            .unwrap_or("<unknown>");
                        self.set_info(format!(
                            "Deleted {} agent from project \"{}\" with branch \"{}\"",
                            session.provider.as_str(),
                            project_name,
                            session.branch_name
                        ));
                    }
                }
                // Guarded by the early return above; kept for exhaustiveness.
                (false, true, None) => {}
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

    fn change_agent_provider_options(
        &self,
        session: &AgentSession,
    ) -> Vec<ChangeAgentProviderOption> {
        self.config
            .providers
            .commands
            .keys()
            .map(|name| {
                let provider = ProviderKind::new(name.clone());
                let cfg = provider_config(&self.config, &provider);
                let supports_resume = cfg.supports_session_resume();
                let resume_available = supports_resume && session.has_started_provider(&provider);
                ChangeAgentProviderOption {
                    is_current: provider == session.provider,
                    provider,
                    supports_resume,
                    resume_available,
                }
            })
            .collect()
    }

    pub(crate) fn open_change_agent_provider_prompt(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };
        if self.config.providers.commands.is_empty() {
            self.set_error("No providers are configured.");
            return Ok(());
        }
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ChangeAgentProvider(ChangeAgentProviderPrompt {
            session_id: session.id.clone(),
            session_label: self.session_label(&session),
            worktree_path: session.worktree_path.clone(),
            options: self.change_agent_provider_options(&session),
            selected: 0,
            focus: ChangeAgentProviderFocus::List,
        });
        self.set_info(
            "Choose a provider for this worktree. The change takes effect on the next launch; dux resumes each provider's prior session on this worktree when available.",
        );
        Ok(())
    }

    pub(crate) fn apply_change_agent_provider(&mut self) -> Result<()> {
        let prompt = match &self.prompt {
            PromptState::ChangeAgentProvider(prompt) => prompt.clone(),
            _ => return Ok(()),
        };
        let Some(selected) = prompt.options.get(prompt.selected).cloned() else {
            self.prompt = PromptState::None;
            self.set_error("Select a provider first.");
            return Ok(());
        };
        let Some(session_index) = self
            .sessions
            .iter()
            .position(|session| session.id == prompt.session_id)
        else {
            self.prompt = PromptState::None;
            self.set_error("The selected agent is no longer available.");
            return Ok(());
        };

        if selected.is_current {
            self.prompt = PromptState::None;
            self.set_info(format!(
                "Agent \"{}\" already uses {}. Pick another provider to swap.",
                prompt.session_label,
                selected.provider.as_str(),
            ));
            return Ok(());
        }

        self.prompt = PromptState::None;

        let session_id = self.sessions[session_index].id.clone();
        let running = self.providers.contains_key(&session_id);
        let previous_provider = self.sessions[session_index].provider.clone();

        let session = &mut self.sessions[session_index];
        session.provider = selected.provider.clone();
        session.updated_at = Utc::now();
        let updated = session.clone();
        self.session_store.upsert_session(&updated)?;

        // Pin the still-running provider so UI labels stay truthful until
        // the user exits and relaunches the agent. Only set on the first
        // swap-while-running — later swaps don't change what's spawned.
        if running {
            self.running_provider_pins
                .entry(session_id.clone())
                .or_insert(previous_provider.clone());
        }
        self.rebuild_left_items();

        let reconnect_key = self.bindings.label_for(Action::ReconnectAgent);
        if running {
            self.set_warning(format!(
                "Worktree \"{}\" is set to {}, but the {} agent is still running. Exit it and press {} to relaunch with {}.",
                prompt.session_label,
                selected.provider.as_str(),
                previous_provider.as_str(),
                reconnect_key,
                selected.provider.as_str(),
            ));
        } else {
            let resume_note = if selected.resume_available {
                " dux will resume its prior session on this worktree."
            } else {
                " This provider hasn't run on this worktree yet, so it'll start a fresh session."
            };
            self.set_info(format!(
                "Worktree \"{}\" will use {} next launch. Press {} to start it.{}",
                prompt.session_label,
                selected.provider.as_str(),
                reconnect_key,
                resume_note,
            ));
        }
        Ok(())
    }

    fn change_default_provider_options(&self) -> Vec<ChangeDefaultProviderOption> {
        let current = self.config.default_provider();
        self.config
            .providers
            .commands
            .keys()
            .map(|name| {
                let provider = ProviderKind::new(name.clone());
                ChangeDefaultProviderOption {
                    is_current: provider == current,
                    provider,
                }
            })
            .collect()
    }

    pub(crate) fn open_change_default_provider_prompt(&mut self) -> Result<()> {
        if self.config.providers.commands.is_empty() {
            self.set_error("No providers are configured.");
            return Ok(());
        }
        let options = self.change_default_provider_options();
        let selected = options
            .iter()
            .position(|option| option.is_current)
            .unwrap_or(0);
        let current = self.config.default_provider();
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ChangeDefaultProvider(ChangeDefaultProviderPrompt {
            current,
            options,
            selected,
            focus: ChangeDefaultProviderFocus::List,
        });
        self.set_info(
            "Choose the default provider for newly created agent sessions. Existing agents keep their current provider.",
        );
        Ok(())
    }

    pub(crate) fn apply_change_default_provider(&mut self) -> Result<()> {
        let prompt = match &self.prompt {
            PromptState::ChangeDefaultProvider(prompt) => prompt.clone(),
            _ => return Ok(()),
        };
        let Some(selected) = prompt.options.get(prompt.selected).cloned() else {
            self.prompt = PromptState::None;
            self.set_error("Select a provider first.");
            return Ok(());
        };
        self.prompt = PromptState::None;
        if selected.is_current {
            self.set_info(format!(
                "{} is already the default provider. Pick a different one to change it.",
                selected.provider.as_str(),
            ));
            return Ok(());
        }
        let previous = self.config.defaults.provider.clone();
        self.config.defaults.provider = selected.provider.as_str().to_string();
        if let Err(err) = save_config(&self.paths.config_path, &self.config, &self.bindings) {
            self.config.defaults.provider = previous;
            self.set_error(format!(
                "Couldn't persist the default provider change: {err:#}"
            ));
            return Ok(());
        }
        refresh_project_defaults(&mut self.projects, &self.config);
        self.rebuild_left_items();
        self.set_info(format!(
            "Default provider changed to {}. New agent sessions will use it; existing agents keep their current provider. Use \"change-agent-provider\" on a session to switch providers for an existing worktree.",
            selected.provider.as_str(),
        ));
        Ok(())
    }

    pub(crate) fn open_change_theme_prompt(&mut self) -> Result<()> {
        let options = crate::theme::discover_available(&self.paths);
        if options.is_empty() {
            self.set_error("No themes available.");
            return Ok(());
        }
        let current = self.config.ui.theme.clone();
        let selected = options
            .iter()
            .position(|opt| opt.id == current)
            .unwrap_or(0);
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ChangeTheme(ChangeThemePrompt {
            options,
            selected,
            current,
        });
        self.set_info(
            "Themes preview live as you move. Enter saves the choice; Esc reverts to the previous theme.",
        );
        Ok(())
    }

    /// Live-preview the theme at the prompt's current selection. Called every
    /// time the user moves the cursor in the picker (keyboard or mouse) so
    /// the whole UI repaints with the highlighted theme without having to
    /// commit anything yet. Failures are swallowed — a theme that won't load
    /// just leaves the previously-previewed theme in place; the picker stays
    /// open so the user can pick a different one.
    pub(crate) fn preview_change_theme_selection(&mut self) {
        let id = match &self.prompt {
            PromptState::ChangeTheme(prompt) => prompt
                .options
                .get(prompt.selected)
                .map(|option| option.id.clone()),
            _ => None,
        };
        let Some(id) = id else { return };
        if let Ok(theme) = crate::theme::load(&id, &self.paths) {
            self.theme = theme;
        }
    }

    /// Cancel the theme picker. Reloads the theme that was active when the
    /// picker opened so any live previews are reverted.
    pub(crate) fn cancel_change_theme(&mut self) {
        let original = match &self.prompt {
            PromptState::ChangeTheme(prompt) => Some(prompt.current.clone()),
            _ => None,
        };
        self.prompt = PromptState::None;
        if let Some(original) = original
            && let Ok(theme) = crate::theme::load(&original, &self.paths)
        {
            self.theme = theme;
        }
    }

    pub(crate) fn apply_change_theme(&mut self) -> Result<()> {
        let prompt = match &self.prompt {
            PromptState::ChangeTheme(prompt) => prompt.clone(),
            _ => return Ok(()),
        };
        let Some(selected) = prompt.options.get(prompt.selected).cloned() else {
            self.prompt = PromptState::None;
            self.set_error("Select a theme first.");
            return Ok(());
        };
        self.prompt = PromptState::None;
        if selected.id == prompt.current {
            self.set_info(format!(
                "Theme \"{}\" is already active. Pick a different one to change it.",
                selected.display_name,
            ));
            return Ok(());
        }
        let theme = match crate::theme::load(&selected.id, &self.paths) {
            Ok(theme) => theme,
            Err(err) => {
                self.set_error(format!(
                    "Couldn't load theme \"{}\": {err:#}",
                    selected.display_name
                ));
                return Ok(());
            }
        };
        let previous = self.config.ui.theme.clone();
        self.config.ui.theme = selected.id.clone();
        if let Err(err) = save_config(&self.paths.config_path, &self.config, &self.bindings) {
            self.config.ui.theme = previous;
            self.set_error(format!(
                "Couldn't persist the theme change: {err:#}. The new theme is loaded for this session only."
            ));
            // Still apply to the running session — the user explicitly asked
            // for it and we'd rather flash a wrong-color UI than silently
            // ignore the request.
            self.theme = theme;
            return Ok(());
        }
        self.theme = theme;
        self.set_info(format!(
            "Theme changed to \"{}\". Future sessions will use it too.",
            selected.display_name,
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

        // Refuse if any of this project's sessions have an async worktree
        // removal in-flight. `do_delete_session` runs git synchronously and
        // would race the worker, potentially leaving the project deletion
        // half-finished. The user must wait for the worker to complete (or
        // fail) before retrying.
        let pending_in_project = self
            .sessions
            .iter()
            .any(|s| s.project_id == project.id && self.pending_deletions.contains(&s.id));
        if pending_in_project {
            self.set_error(
                "Cannot delete project while agent worktree removals are in progress. \
                 Wait for them to finish, then try again.",
            );
            return Ok(());
        }

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
                // When deleting a project we also remove each agent's
                // worktree — the project itself is going away, so leaving
                // orphaned worktrees around would be surprising.
                self.do_delete_session(&session_id, true)?;
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
        self.running_provider_pins.remove(&session.id);
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
                self.mark_session_provider_started(&session.id);
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

        let use_resume = self.should_resume_session(&session);
        match self.spawn_pty_for_session(&session, use_resume) {
            Ok(client) => {
                self.providers.insert(session.id.clone(), client);
                if use_resume {
                    self.resume_fallback_candidates
                        .insert(session.id.clone(), Instant::now());
                }
                self.mark_session_status(&session.id, SessionStatus::Active);
                self.mark_session_provider_started(&session.id);
                self.show_agent_surface();
                self.input_target = InputTarget::Agent;
                self.fullscreen_overlay = FullscreenOverlay::Agent;
                let proj_name = self.project_name_for_session(&session);
                let mut msg = if use_resume {
                    format!(
                        "Resumed {} agent \"{}\" in project \"{}\".",
                        session.provider.as_str(),
                        session.branch_name,
                        proj_name
                    )
                } else {
                    format!(
                        "Started fresh {} session for agent \"{}\" in project \"{}\". Use /sessions inside the agent to restore a prior conversation.",
                        session.provider.as_str(),
                        session.branch_name,
                        proj_name
                    )
                };
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
                        self.running_provider_pins.remove(session_id);
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
        self.running_provider_pins.remove(&conflicting.id);
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
            running_provider_pins: std::collections::HashMap::new(),
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
            pressed_button: None,
            interactive_patterns: crate::keybindings::InteractiveBytePatterns {
                bindings: Vec::new(),
            },
            raw_input_buf: Vec::new(),
            loading_input_buf: Vec::new(),
            in_bracket_paste: false,
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
            resume_fallback_candidates: std::collections::HashMap::new(),
            pending_deletions: std::collections::HashSet::new(),
            deletion_busy_messages: std::collections::HashMap::new(),
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
            started_providers: Vec::new(),
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

    #[test]
    fn should_resume_only_for_providers_started_on_session() {
        let mut session = make_session("s1", "claude", "/tmp/wt/a");
        session.started_providers = vec!["claude".to_string()];
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session.clone()], vec![project]);

        assert!(app.should_resume_session(&session));

        app.sessions[0].provider = ProviderKind::from_str("codex");
        let session = app.sessions[0].clone();
        assert!(!app.should_resume_session(&session));

        app.sessions[0].started_providers.push("codex".to_string());
        let session = app.sessions[0].clone();
        assert!(app.should_resume_session(&session));
    }

    #[test]
    fn mark_session_provider_started_persists_history() {
        let session = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session], vec![project]);

        app.mark_session_provider_started("s1");

        assert_eq!(
            app.sessions[0].started_providers,
            vec!["claude".to_string()]
        );
        let persisted = app.session_store.load_sessions().expect("load sessions");
        assert_eq!(persisted[0].started_providers, vec!["claude".to_string()]);
    }

    /// Build a `Project` whose `path` points at a caller-controlled directory,
    /// so tests can decide whether git operations succeed or fail.
    fn make_project_at(id: &str, provider: &str, path: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "demo".to_string(),
            path: path.to_string(),
            default_provider: ProviderKind::from_str(provider),
            current_branch: "main".to_string(),
            path_missing: false,
        }
    }

    /// With `delete_worktree = false`, the session record is removed but the
    /// worktree on disk is left alone and git is never invoked. The project
    /// path here is not a git repo — if the code accidentally invoked git it
    /// would return `Err` and this test would catch it.
    #[test]
    fn do_delete_session_preserves_worktree_when_flag_off() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.do_delete_session("s1", false)
            .expect("delete should succeed without touching git");

        assert!(
            app.sessions.iter().all(|s| s.id != "s1"),
            "session should be removed"
        );
        assert!(
            worktree_dir.path().exists(),
            "worktree directory must be preserved on disk when delete_worktree=false",
        );
    }

    /// When another session shares the worktree, the worktree must be
    /// preserved even if the user checked "also delete the worktree" — other
    /// sessions still depend on it. Git must not be invoked.
    #[test]
    fn do_delete_session_keeps_shared_worktree_even_when_flag_on() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        let mut s2 = make_session("s2", "codex", &worktree_path);
        s1.project_id = "project-1".to_string();
        s2.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);

        app.do_delete_session("s1", true)
            .expect("delete should succeed without touching git for shared worktree");

        assert!(
            app.sessions.iter().all(|s| s.id != "s1"),
            "s1 should be removed"
        );
        assert!(
            app.sessions.iter().any(|s| s.id == "s2"),
            "s2 should remain"
        );
        assert!(
            worktree_dir.path().exists(),
            "shared worktree must be preserved when siblings exist",
        );
    }

    /// If git fails to remove the worktree, the session record must remain —
    /// otherwise the user loses their agent with no way to retry. We force
    /// the git call to fail by pointing the project path at a directory that
    /// is not a git repository.
    #[test]
    fn do_delete_session_preserves_session_when_git_fails() {
        let project_dir = tempdir().expect("project tempdir");
        // Intentionally NOT a git repo — `git worktree remove` will exit
        // non-zero, which bubbles up as Err from git::remove_worktree.
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        let err = app
            .do_delete_session("s1", true)
            .expect_err("git should fail against a non-git project dir");
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("worktree") || msg.contains("git"),
            "error should mention git/worktree, got: {msg}",
        );

        assert!(
            app.sessions.iter().any(|s| s.id == "s1"),
            "session must be preserved when git fails so user can retry",
        );
        assert!(
            worktree_dir.path().exists(),
            "worktree directory should be untouched on failure",
        );
    }

    /// The async path (`begin_delete_session`) must NOT remove the session
    /// immediately when the worktree deletion is going to run in a worker —
    /// otherwise a failed git call would leave the user with a vanished
    /// agent and no way to retry. The session should only disappear once
    /// the worker reports success.
    #[test]
    fn begin_delete_session_keeps_session_until_worker_replies() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", true);

        // Session must still be present: the worker thread has been spawned
        // but hasn't (at most) completed the cleanup on our thread yet.
        assert!(
            app.sessions.iter().any(|s| s.id == "s1"),
            "session must remain until the worker reports success",
        );
    }

    /// When the async path does NOT need to run git (no siblings + flag off),
    /// cleanup is safe to run inline and the session should be gone by the
    /// time `begin_delete_session` returns.
    #[test]
    fn begin_delete_session_completes_inline_when_no_git_needed() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", false);

        assert!(
            app.sessions.iter().all(|s| s.id != "s1"),
            "no-git path should complete immediately",
        );
        assert!(
            worktree_dir.path().exists(),
            "worktree directory must be preserved when the flag is off",
        );
    }

    /// `finish_delete_session` is the handler invoked both inline and from
    /// the worker event. It must be idempotent: if the session has already
    /// been removed (e.g. a duplicate worker event) it should no-op.
    #[test]
    fn finish_delete_session_is_idempotent() {
        let mut s1 = make_session("s1", "claude", "/tmp/wt/a");
        s1.project_id = "project-1".to_string();
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.finish_delete_session("s1", false, None, true)
            .expect("first finish succeeds");
        // Second call must not panic or return Err even though session is gone.
        app.finish_delete_session("s1", false, None, true)
            .expect("second finish is a no-op");
    }

    /// Kicking off the async delete path should mark the session as
    /// pending so the UI can dim the row.
    #[test]
    fn begin_delete_session_tracks_pending_deletion() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", true);

        assert!(
            app.pending_deletions.contains("s1"),
            "session must be marked pending while async worker runs",
        );
    }

    /// The inline (no-git) path completes immediately, so pending_deletions
    /// should never gain the session in the first place.
    #[test]
    fn begin_delete_session_inline_does_not_track() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", false);

        assert!(
            app.pending_deletions.is_empty(),
            "inline path should never populate pending_deletions",
        );
    }

    /// A second delete request for a session that's already being deleted
    /// must be refused with an error, and must NOT spawn another worker
    /// (i.e. the pending-deletions set size stays at 1).
    #[test]
    fn begin_delete_session_rejects_duplicate_request() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", true);
        assert_eq!(app.pending_deletions.len(), 1, "first call records pending");

        app.begin_delete_session("s1", true);
        assert_eq!(
            app.pending_deletions.len(),
            1,
            "duplicate request must not spawn a second worker",
        );
    }

    /// If the session was removed by another code path while the async
    /// delete worker was running, the worker's completion event must still
    /// overwrite the Busy status line when the message matches.
    #[test]
    fn worktree_remove_completed_clears_busy_when_session_already_gone() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        // Simulate the Busy state set by `begin_delete_session`, including
        // the tracking map entry.
        let busy_msg = "Removing worktree for agent \"branch-s1\"\u{2026}";
        app.set_busy(busy_msg);
        app.pending_deletions.insert("s1".to_string());
        app.deletion_busy_messages
            .insert("s1".to_string(), busy_msg.to_string());

        // Another code path removes the session before the worker replies.
        app.sessions.retain(|s| s.id != "s1");

        // The worker then reports success.
        app.worker_tx
            .send(WorkerEvent::WorktreeRemoveCompleted {
                session_id: "s1".to_string(),
                result: Ok(false),
            })
            .expect("channel send");
        app.drain_events();

        assert!(
            app.pending_deletions.is_empty(),
            "pending guard must be cleared on completion",
        );
        assert_ne!(
            app.status.tone(),
            crate::statusline::StatusTone::Busy,
            "Busy status must not linger after worker completes, got: {}",
            app.status.text(),
        );
    }

    /// When the session is already gone AND the status line has already been
    /// overwritten by a later Info action (e.g. project deletion), the
    /// worker completion should not clobber the newer message.
    #[test]
    fn worktree_remove_completed_does_not_clobber_newer_info() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.pending_deletions.insert("s1".to_string());
        app.deletion_busy_messages
            .insert("s1".to_string(), "Removing worktree\u{2026}".to_string());
        app.sessions.retain(|s| s.id != "s1");

        // Another action already set a non-Busy status.
        app.set_info("Deleted project \"demo\" and all its agents");

        app.worker_tx
            .send(WorkerEvent::WorktreeRemoveCompleted {
                session_id: "s1".to_string(),
                result: Ok(false),
            })
            .expect("channel send");
        app.drain_events();

        assert_eq!(
            app.status.tone(),
            crate::statusline::StatusTone::Info,
            "tone should remain Info",
        );
        assert!(
            app.status.text().contains("Deleted project"),
            "the project-deletion message must not be clobbered, got: {}",
            app.status.text(),
        );
    }

    /// When the session is already gone AND the status line shows a Busy
    /// message from an *unrelated* operation (push, pull, etc.), the worker
    /// completion should not clobber it — the message text doesn't match
    /// ours, even though the tone is also Busy.
    #[test]
    fn worktree_remove_completed_does_not_clobber_unrelated_busy() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.pending_deletions.insert("s1".to_string());
        app.deletion_busy_messages.insert(
            "s1".to_string(),
            "Removing worktree for agent \"branch-s1\"\u{2026}".to_string(),
        );
        app.sessions.retain(|s| s.id != "s1");

        // An unrelated operation set its own Busy message.
        app.set_busy("Pushing to remote\u{2026}");

        app.worker_tx
            .send(WorkerEvent::WorktreeRemoveCompleted {
                session_id: "s1".to_string(),
                result: Ok(false),
            })
            .expect("channel send");
        app.drain_events();

        // The status should still show the push Busy, not "Worktree removal
        // finished."
        assert_eq!(
            app.status.tone(),
            crate::statusline::StatusTone::Busy,
            "tone should remain Busy from the push",
        );
        assert_eq!(
            app.status.message(),
            "Pushing to remote\u{2026}",
            "the push message must not be clobbered, got: {}",
            app.status.message(),
        );
    }

    /// Project deletion must be refused when any of the project's sessions
    /// have an async worktree removal in-flight. Allowing it would race the
    /// synchronous `do_delete_session` against the worker and could leave the
    /// project half-deleted with an orphaned worktree.
    #[test]
    fn delete_selected_project_blocked_when_pending() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        // Simulate an async delete in-flight for this session.
        app.pending_deletions.insert("s1".to_string());

        // The project is the first item in the list, select it.
        app.selected_left = 0;

        app.delete_selected_project()
            .expect("should return Ok (error reported via status line)");

        // Session must still be present — deletion was refused.
        assert!(
            app.sessions.iter().any(|s| s.id == "s1"),
            "session must not be removed when deletion is blocked",
        );
        assert!(
            app.projects.iter().any(|p| p.id == "project-1"),
            "project must not be removed when deletion is blocked",
        );
        assert_eq!(
            app.status.tone(),
            crate::statusline::StatusTone::Error,
            "should show an error explaining why deletion was blocked",
        );
    }

    /// When the worker fails to delete a worktree, the error message should
    /// include the agent label so the user knows which one failed.
    #[test]
    fn worktree_remove_failure_identifies_agent() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.pending_deletions.insert("s1".to_string());

        app.worker_tx
            .send(WorkerEvent::WorktreeRemoveCompleted {
                session_id: "s1".to_string(),
                result: Err("fatal: not a git repository".to_string()),
            })
            .expect("channel send");
        app.drain_events();

        let msg = app.status.text();
        assert!(
            msg.contains("branch-s1"),
            "error should include the agent's branch name, got: {msg}",
        );
        assert!(
            msg.contains("not a git repository"),
            "error should include the git error, got: {msg}",
        );
    }
}
