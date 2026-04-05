use super::*;

impl App {
    pub(crate) fn drain_events(&mut self) {
        while let Ok(event) = self.worker_rx.try_recv() {
            match event {
                WorkerEvent::CreateAgentProgress(message) => self.set_busy(message),
                WorkerEvent::CreateAgentReady(boxed) => {
                    let AgentReadyData {
                        session,
                        client,
                        pty_size,
                        status_message,
                    } = *boxed;
                    self.create_agent_in_flight = false;
                    self.last_pty_size = pty_size;
                    if let Err(err) = self.session_store.upsert_session(&session) {
                        logger::error(&format!(
                            "session store upsert failed for {}: {err}",
                            session.id
                        ));
                        self.set_error(format!("Failed to persist session: {err}"));
                        continue;
                    }
                    self.providers.insert(session.id.clone(), client);
                    self.sessions.insert(0, session.clone());
                    self.rebuild_left_items();
                    self.selected_left = self
                        .left_items()
                        .iter()
                        .position(|item| matches!(item, LeftItem::Session(index) if self.sessions.get(*index).map(|candidate| candidate.id.as_str()) == Some(session.id.as_str())))
                        .unwrap_or(0);
                    self.reload_changed_files();
                    self.show_agent_surface();
                    self.input_target = InputTarget::Agent;
                    self.fullscreen_overlay = FullscreenOverlay::Agent;
                    self.set_info(status_message);
                }
                WorkerEvent::CreateAgentFailed(message) => {
                    self.create_agent_in_flight = false;
                    self.set_error(message);
                }
                WorkerEvent::ChangedFilesReady { staged, unstaged } => {
                    self.staged_files = staged;
                    self.unstaged_files = unstaged;
                    self.clamp_files_cursor();
                }
                WorkerEvent::CommitMessageGenerated(msg) => {
                    self.commit_input.clear_overlay();
                    self.commit_input.set_text(msg);
                    self.input_target = InputTarget::CommitMessage;
                    {
                        let exit_key = self.bindings.label_for(Action::ExitCommitInput);
                        let commit_key = self.bindings.label_for(Action::CommitChanges);
                        self.set_info(format!(
                            "AI commit message generated. Press {exit_key} to exit, then {commit_key} to commit.",
                        ));
                    }
                }
                WorkerEvent::CommitMessageFailed(err) => {
                    self.commit_input.clear_overlay();
                    {
                        let gen_key = self.bindings.label_for(Action::GenerateCommitMessage);
                        self.set_error(format!(
                            "Failed to generate AI commit message: {err}. \
                             You can write one manually or retry with {gen_key}.",
                        ));
                    }
                }
                WorkerEvent::PushCompleted(result) => match result {
                    Ok(()) => self.set_info(
                        "Pushed to remote successfully. Your changes are now available to collaborators.",
                    ),
                    Err(e) => self.set_error(format!("Push to remote failed: {e}")),
                },
                WorkerEvent::PullCompleted(result) => match result {
                    Ok(()) => {
                        self.set_info(
                            "Pulled latest changes from remote successfully. Local branch is up to date.",
                        );
                        self.reload_changed_files();
                    }
                    Err(e) => self.set_error(format!("Pull from remote failed: {e}")),
                },
                WorkerEvent::BrowserEntriesReady { dir, entries } => {
                    if let PromptState::BrowseProjects {
                        current_dir,
                        entries: current_entries,
                        loading,
                        selected,
                        ..
                    } = &mut self.prompt
                        && *current_dir == dir
                    {
                        *current_entries = entries;
                        *loading = false;
                        *selected = 0;
                    }
                }
            }
        }
        // Detect PTY exits.
        let mut exited = Vec::new();
        for (session_id, provider) in &mut self.providers {
            if provider.is_exited() || provider.try_wait().is_some() {
                exited.push(session_id.clone());
            }
        }
        for session_id in &exited {
            self.providers.remove(session_id);
            self.mark_session_status(session_id, SessionStatus::Detached);
        }
        if !exited.is_empty() {
            // If the currently-viewed session just exited, leave interactive mode.
            if let Some(current) = self.selected_session()
                && exited.contains(&current.id)
            {
                let key = self.bindings.label_for(Action::ReconnectAgent);
                if self.session_surface == SessionSurface::Agent {
                    self.input_target = InputTarget::None;
                    self.fullscreen_overlay = FullscreenOverlay::None;
                    self.focus = FocusPane::Left;
                    self.set_info(format!(
                        "Agent CLI process has exited. Press \"{key}\" to relaunch."
                    ));
                } else {
                    self.set_info(format!(
                        "Agent CLI process exited. Companion terminal is still available; press \"{key}\" to relaunch the agent."
                    ));
                }
            }
        }

        let mut exited_terminal_ids = Vec::new();
        for (terminal_id, terminal) in &mut self.companion_terminals {
            if terminal.client.is_exited() || terminal.client.try_wait().is_some() {
                exited_terminal_ids.push(terminal_id.clone());
            }
        }
        for terminal_id in &exited_terminal_ids {
            self.companion_terminals.remove(terminal_id);
        }
        if !exited_terminal_ids.is_empty() {
            // If the active terminal just exited, close the overlay.
            if let Some(ref active_id) = self.active_terminal_id
                && exited_terminal_ids.contains(active_id)
            {
                self.active_terminal_id = None;
                if self.input_target == InputTarget::Terminal {
                    self.input_target = InputTarget::None;
                }
                self.fullscreen_overlay = FullscreenOverlay::None;
                self.session_surface = SessionSurface::Agent;
                self.set_info("Terminal exited. Press the terminal key to launch a new one.");
            }
            self.clamp_terminal_cursor();
        }

        // Poll foreground process names every ~2 seconds (every 20 ticks).
        if self.tick_count.is_multiple_of(20) {
            for terminal in self.companion_terminals.values_mut() {
                terminal.foreground_cmd = terminal.client.foreground_process_name();
            }
        }

        // Keep the poller's interval flag in sync with whether any runtime PTY is alive.
        self.has_active_processes
            .store(self.running_process_count() > 0, Ordering::Relaxed);
    }

    pub(crate) fn spawn_browser_entries(&self, dir: &Path) {
        let tx = self.worker_tx.clone();
        let dir = dir.to_path_buf();
        thread::spawn(move || {
            let entries = browser_entries(&dir);
            logger::debug(&format!(
                "browser loaded {} with {} entries",
                dir.display(),
                entries.len()
            ));
            let _ = tx.send(WorkerEvent::BrowserEntriesReady {
                dir: dir.clone(),
                entries,
            });
        });
    }

    pub(crate) fn spawn_changed_files_poller(&self) {
        let tx = self.worker_tx.clone();
        let watched = Arc::clone(&self.watched_worktree);
        let has_agent = Arc::clone(&self.has_active_processes);
        thread::spawn(move || {
            loop {
                let interval = if has_agent.load(Ordering::Relaxed) {
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(10)
                };
                thread::sleep(interval);
                let path = watched.lock().ok().and_then(|guard| guard.clone());
                if let Some(worktree_path) = path
                    && let Ok((staged, unstaged)) = git::changed_files(&worktree_path)
                    && tx
                        .send(WorkerEvent::ChangedFilesReady { staged, unstaged })
                        .is_err()
                {
                    break; // receiver dropped, app is shutting down
                }
            }
        });
    }
}

pub(crate) fn run_create_agent_job(
    request: CreateAgentRequest,
    paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
) {
    let (project, provider, source_branch, status_message, branch_name, worktree_path) =
        match request {
            CreateAgentRequest::NewProject { project } => {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                    "Creating a new worktree for project \"{}\"...",
                    project.name
                )));
                let repo_path = PathBuf::from(&project.path);
                let (branch_name, worktree_path) =
                    match git::create_worktree(&repo_path, &paths.worktrees_root, &project.name) {
                        Ok(result) => result,
                        Err(err) => {
                            logger::error(&format!(
                                "worktree creation failed for {}: {err}",
                                project.path
                            ));
                            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                                "Failed to create a new worktree for project \"{}\": {err}",
                                project.name
                            )));
                            return;
                        }
                    };
                let status_message = format!(
                    "Created {} agent \"{}\" in project \"{}\". The new worktree is ready in a fresh session.",
                    project.default_provider.as_str(),
                    branch_name,
                    project.name
                );
                (
                    project.clone(),
                    project.default_provider.clone(),
                    project.current_branch.clone(),
                    status_message,
                    branch_name,
                    worktree_path,
                )
            }
            CreateAgentRequest::ForkSession {
                project,
                source_session,
                source_label,
            } => {
                let source_worktree = PathBuf::from(&source_session.worktree_path);
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                    "Creating a forked worktree from agent \"{source_label}\"...",
                )));
                let source_head = match git::head_commit(&source_worktree) {
                    Ok(head) => head,
                    Err(err) => {
                        logger::error(&format!(
                            "failed to resolve HEAD for {}: {err}",
                            source_session.worktree_path
                        ));
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to inspect the source worktree for agent \"{source_label}\": {err}",
                    )));
                        return;
                    }
                };
                let repo_path = PathBuf::from(&project.path);
                let (branch_name, worktree_path) = match git::create_worktree_from_start_point(
                    &repo_path,
                    &paths.worktrees_root,
                    &project.name,
                    Some(&source_head),
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        logger::error(&format!(
                            "fork worktree creation failed for {}: {err}",
                            project.path
                        ));
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to create a forked worktree from agent \"{source_label}\": {err}",
                    )));
                        return;
                    }
                };
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Copying the current filesystem contents from agent \"{source_label}\" into the new fork...",
            )));
                if let Err(err) = git::mirror_worktree_contents(&source_worktree, &worktree_path) {
                    logger::error(&format!(
                        "failed to mirror worktree {} into {}: {err}",
                        source_worktree.display(),
                        worktree_path.display()
                    ));
                    let _ = git::remove_worktree(&repo_path, &worktree_path, &branch_name);
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Failed to copy the source worktree contents for agent \"{source_label}\": {err}",
                )));
                    return;
                }
                let status_message = format!(
                    "Forked {} agent \"{}\" from \"{}\" in project \"{}\". The new worktree starts with copied files and a fresh session.",
                    source_session.provider.as_str(),
                    branch_name,
                    source_label,
                    project.name
                );
                (
                    project,
                    source_session.provider,
                    source_session.branch_name,
                    status_message,
                    branch_name,
                    worktree_path,
                )
            }
        };
    let repo_path = PathBuf::from(&project.path);
    logger::info(&format!(
        "created worktree {} on branch {}",
        worktree_path.display(),
        branch_name
    ));
    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
        provider,
        source_branch,
        branch_name,
        worktree_path: worktree_path.to_string_lossy().to_string(),
        title: None,
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let provider_cfg = provider_config(&config, &session.provider);
    if let Err(hint) = check_provider_available(&provider_cfg) {
        logger::error(&format!("provider not found for {}: {hint}", session.id));
        let _ = git::remove_worktree(
            &repo_path,
            Path::new(&session.worktree_path),
            &session.branch_name,
        );
        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(hint));
        return;
    }
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
        "Launching {} in a fresh session...",
        session.provider.as_str()
    )));
    // crossterm::terminal::size() returns (cols, rows).
    let (cols, rows) = term_size;
    let client = match PtyClient::spawn(
        &provider_cfg.command,
        &provider_cfg.args,
        &worktree_path,
        rows,
        cols,
        config.ui.agent_scrollback_lines,
    ) {
        Ok(client) => client,
        Err(err) => {
            logger::error(&format!("PTY spawn failed for {}: {err}", session.id));
            let _ = git::remove_worktree(
                &repo_path,
                Path::new(&session.worktree_path),
                &session.branch_name,
            );
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                "Failed to start {}: {err}",
                provider_cfg.command
            )));
            return;
        }
    };
    logger::info(&format!("PTY session started for {}", session.id));
    let _ = worker_tx.send(WorkerEvent::CreateAgentReady(Box::new(AgentReadyData {
        session,
        client,
        pty_size: (rows, cols),
        status_message,
    })));
}

pub(crate) fn browser_entries(dir: &Path) -> Vec<BrowserEntry> {
    let mut entries = fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let is_git_repo = path.join(".git").exists();
            let label = if is_git_repo {
                name
            } else {
                format!("{name}/")
            };
            Some(BrowserEntry {
                is_git_repo,
                path,
                label,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        b.is_git_repo
            .cmp(&a.is_git_repo)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    if let Some(parent) = dir.parent() {
        entries.insert(
            0,
            BrowserEntry {
                path: parent.to_path_buf(),
                label: "../".to_string(),
                is_git_repo: false,
            },
        );
    }
    entries
}
