use super::*;

impl App {
    pub(crate) fn drain_events(&mut self) {
        while let Ok(event) = self.worker_rx.try_recv() {
            match event {
                WorkerEvent::CreateAgentProgress(message) => self.set_busy(message),
                WorkerEvent::CreateAgentReady(boxed) => {
                    let AgentReadyData { session, client, pty_size } = *boxed;
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
                    self.focus = FocusPane::Center;
                    self.center_mode = CenterMode::Agent;
                    self.input_target = InputTarget::Agent;
                    self.fullscreen_agent = true;
                    let proj_name = self.project_name_for_session(&session);
                    self.set_info(format!(
                        "Created {} agent \"{}\" in project \"{}\"",
                        session.provider.as_str(),
                        session.branch_name,
                        proj_name
                    ));
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
                    self.commit_input = msg;
                    self.commit_input_cursor = self.commit_input.len();
                    self.commit_generating = false;
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
                    self.commit_generating = false;
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
                    {
                        if *current_dir == dir {
                            *current_entries = entries;
                            *loading = false;
                            *selected = 0;
                        }
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
            if let Some(current) = self.selected_session() {
                if exited.contains(&current.id) {
                    self.input_target = InputTarget::None;
                    self.fullscreen_agent = false;
                    self.focus = FocusPane::Left;
                    let key = self.bindings.label_for(Action::ReconnectAgent);
                    self.set_info(format!(
                        "Agent CLI process has exited. Press \"{key}\" to relaunch."
                    ));
                }
            }
        }
        // Keep the poller's interval flag in sync with whether any agent is running.
        self.has_active_agent
            .store(!self.providers.is_empty(), Ordering::Relaxed);
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
        let has_agent = Arc::clone(&self.has_active_agent);
        thread::spawn(move || {
            loop {
                let interval = if has_agent.load(Ordering::Relaxed) {
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(10)
                };
                thread::sleep(interval);
                let path = watched.lock().ok().and_then(|guard| guard.clone());
                if let Some(worktree_path) = path {
                    if let Ok((staged, unstaged)) = git::changed_files(&worktree_path) {
                        if tx
                            .send(WorkerEvent::ChangedFilesReady { staged, unstaged })
                            .is_err()
                        {
                            break; // receiver dropped, app is shutting down
                        }
                    }
                }
            }
        });
    }
}

pub(crate) fn run_create_agent_job(
    project: Project,
    paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
) {
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
        "Creating worktree for project \"{}\"...",
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
                    "Worktree creation failed: {err}"
                )));
                return;
            }
        };
    logger::info(&format!(
        "created worktree {} on branch {}",
        worktree_path.display(),
        branch_name
    ));
    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
        provider: project.default_provider.clone(),
        source_branch: project.current_branch.clone(),
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
        "Launching {}...",
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
