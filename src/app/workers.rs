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
                    self.detach_conflicting_worktree_session(
                        &session.worktree_path,
                        &session.id,
                    );
                    self.providers.insert(session.id.clone(), client);
                    self.sessions.insert(0, session.clone());
                    self.mark_session_provider_started(&session.id);
                    self.update_branch_sync_sessions();
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
                WorkerEvent::PullCompleted {
                    repo_path,
                    target,
                    result,
                } => {
                    self.pulls_in_flight.remove(&repo_path);
                    match target {
                        PullTarget::Project {
                            project_id,
                            project_name,
                        } => match result {
                            Ok(branch_name) => {
                                if let Some(existing) = self
                                    .projects
                                    .iter_mut()
                                    .find(|candidate| candidate.id == project_id)
                                    && let Some(branch_name) = branch_name
                                {
                                    existing.current_branch = branch_name;
                                }
                                self.set_info(format!(
                                    "Refreshed project \"{}\". Local branch is up to date with remote.",
                                    project_name,
                                ));
                            }
                            Err(e) => self
                                .set_error(format!("Project refresh failed for \"{}\": {e}", project_name)),
                        },
                        PullTarget::Session => match result {
                            Ok(_) => {
                                self.set_info(
                                    "Pulled latest changes from remote successfully. Local branch is up to date.",
                                );
                                self.reload_changed_files();
                            }
                            Err(e) => self.set_error(format!("Pull from remote failed: {e}")),
                        },
                    }
                }
                WorkerEvent::ClipboardCopyCompleted { label, result } => match result {
                    Ok(()) => self.set_info(label),
                    Err(e) => self.set_error(format!("Clipboard copy failed: {e}")),
                },
                WorkerEvent::BranchRenameCompleted {
                    session_id,
                    new_branch,
                    previous_title,
                    result,
                } => match result {
                    Ok(()) => {
                        if let Some(session) =
                            self.sessions.iter_mut().find(|s| s.id == session_id)
                        {
                            session.branch_name = new_branch.clone();
                            session.updated_at = Utc::now();
                            let _ = self.session_store.upsert_session(session);
                        }
                        self.update_branch_sync_sessions();
                        self.rebuild_left_items();
                        self.set_info(format!(
                            "Renamed agent and branch to \"{new_branch}\"."
                        ));
                    }
                    Err(e) => {
                        // Revert the title so the session doesn't stay in a
                        // mixed state where the display name changed but the
                        // branch didn't.
                        if let Some(session) =
                            self.sessions.iter_mut().find(|s| s.id == session_id)
                        {
                            session.title = previous_title;
                            session.updated_at = Utc::now();
                            let _ = self.session_store.upsert_session(session);
                        }
                        self.rebuild_left_items();
                        self.set_error(format!(
                            "Branch rename failed, reverted agent name: {e}"
                        ));
                    }
                },
                WorkerEvent::BranchSyncReady(updates) => {
                    let mut changed = false;
                    for (session_id, actual_branch) in updates {
                        if let Some(session) =
                            self.sessions.iter_mut().find(|s| s.id == session_id)
                            && session.branch_name != actual_branch {
                                logger::info(&format!(
                                    "branch sync: session {} branch changed {} -> {}",
                                    session_id, session.branch_name, actual_branch,
                                ));
                                session.branch_name = actual_branch;
                                session.updated_at = Utc::now();
                                let _ = self.session_store.upsert_session(session);
                                changed = true;
                            }
                    }
                    if changed {
                        self.update_branch_sync_sessions();
                        self.rebuild_left_items();
                    }
                }
                WorkerEvent::GhStatusChecked(status) => {
                    self.gh_status = status;
                    if matches!(status, crate::model::GhStatus::Available)
                        && self.github_integration_enabled
                    {
                        logger::info("[gh-integration] gh CLI is available and authenticated");
                        self.update_pr_sync_sessions();
                        self.spawn_pr_sync_worker();
                        self.spawn_initial_pr_refresh();
                        self.spawn_refs_watcher();
                    } else {
                        logger::info(&format!(
                            "[gh-integration] gh status: {:?}, integration enabled: {}",
                            status, self.github_integration_enabled,
                        ));
                    }
                }
                WorkerEvent::PrStatusReady(results) => {
                    let now = Instant::now();
                    let mut changed = false;
                    for (session_id, maybe_pr) in results {
                        self.pr_last_checked.insert(session_id.clone(), now);
                        match maybe_pr {
                            Some(pr) => {
                                // Persist the PR association (including state) so it
                                // survives restarts and squash-merge branch deletions.
                                let state_str = match pr.state {
                                    crate::model::PrState::Open => "OPEN",
                                    crate::model::PrState::Merged => "MERGED",
                                    crate::model::PrState::Closed => "CLOSED",
                                };
                                let _ = self.session_store.upsert_pr(
                                    &session_id,
                                    pr.number,
                                    &pr.owner_repo,
                                    state_str,
                                    &pr.title,
                                );
                                self.pr_statuses.insert(session_id, pr);
                                changed = true;
                            }
                            None => {
                                if self.pr_statuses.remove(&session_id).is_some() {
                                    changed = true;
                                }
                            }
                        }
                    }
                    if changed {
                        // Refresh the sync entries so the worker has updated known_pr data.
                        self.update_pr_sync_sessions();
                        self.rebuild_left_items();
                    }
                }
                WorkerEvent::RefsChanged(session_id) => {
                    logger::debug(&format!(
                        "[gh-integration] refs watcher: triggering PR check for session {}",
                        session_id,
                    ));
                    self.spawn_pr_check_for_session(&session_id);
                }
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
                WorkerEvent::WorktreeRemoveCompleted { session_id, result } => {
                    // Always clear the in-flight guard so the session is
                    // interactive again — whether we're about to remove it
                    // (Ok path) or leave it in place for retry (Err path).
                    self.pending_deletions.remove(&session_id);

                    // Retrieve (and remove) the exact Busy message we set
                    // when the worker was spawned. We compare this against
                    // the current status-line content rather than checking
                    // tone alone, because another operation (push, pull,
                    // refresh, concurrent delete) may have since set its own
                    // Busy message that we must not clobber.
                    let our_busy_msg = self.deletion_busy_messages.remove(&session_id);

                    match result {
                        Ok(branch_already_deleted) => {
                            // Only update the status line if the current
                            // content is still the Busy message we set when
                            // spawning this worker. If another operation
                            // (push, pull, concurrent delete) has since
                            // overwritten it, we should not clobber their
                            // message — the session will visually disappear
                            // from the list, which is sufficient feedback.
                            let our_busy_still_showing =
                                our_busy_msg.as_ref().is_some_and(|msg| {
                                    self.status.tone()
                                        == crate::statusline::StatusTone::Busy
                                        && self.status.message() == msg.as_str()
                                });

                            if self.sessions.iter().any(|s| s.id == session_id) {
                                if let Err(e) = self.finish_delete_session(
                                    &session_id,
                                    true,
                                    Some(branch_already_deleted),
                                    our_busy_still_showing,
                                ) {
                                    self.set_error(format!(
                                        "Worktree removed but session cleanup failed: {e:#}"
                                    ));
                                }
                            } else if our_busy_still_showing {
                                // Session removed by another path; just clear
                                // the lingering Busy so it doesn't stick.
                                self.set_info("Worktree removal finished.");
                            }
                        }
                        Err(msg) => {
                            // Session record is normally still present
                            // because we deferred cleanup until git
                            // succeeded. Look up the session label so the
                            // user knows which agent failed — multiple async
                            // deletes can be in flight concurrently, and a
                            // bare error would be ambiguous.
                            if let Some(session) =
                                self.sessions.iter().find(|s| s.id == session_id)
                            {
                                let name = session
                                    .title
                                    .as_deref()
                                    .unwrap_or(&session.branch_name);
                                self.set_error(format!(
                                    "Worktree delete failed for {} agent \"{name}\": {msg}",
                                    session.provider.as_str(),
                                ));
                            } else {
                                self.set_error(format!(
                                    "Worktree delete failed: {msg}"
                                ));
                            }
                        }
                    }
                }
                WorkerEvent::ResourceStatsReady(stats) => {
                    self.resource_stats_in_flight = false;
                    if let PromptState::ResourceMonitor {
                        rows,
                        selected_row,
                        expanded,
                        last_refresh,
                        first_sample,
                        ..
                    } = &mut self.prompt
                    {
                        *rows = stats;
                        *last_refresh = Instant::now();
                        *first_sample = false;
                        // Clamp cursor to the (possibly changed) visual row count.
                        let visual = build_visual_rows(rows, expanded);
                        let max_row = visual.len().saturating_sub(1);
                        if *selected_row > max_row {
                            *selected_row = max_row;
                        }
                    }
                }
            }
        }
        self.retry_hung_resume_sessions();
        // Detect PTY exits.
        let mut exited = Vec::new();
        for (session_id, provider) in &mut self.providers {
            if provider.is_exited() || provider.try_wait().is_some() {
                exited.push(session_id.clone());
            }
        }

        // For sessions that were spawned with resume_args and exited before
        // producing any output, retry with regular args (fresh session).
        // This handles `claude --continue || claude` style fallback.
        let mut retried = HashSet::new();
        for session_id in &exited {
            if self.resume_fallback_candidates.remove(session_id).is_none() {
                continue;
            }
            // Check whether the exited process produced only minimal output
            // (no scrollback and ≤5 visible lines). A failed `--continue`
            // typically prints 1-2 lines of error; a real session produces
            // far more output and scrollback history.
            let is_minimal = self
                .providers
                .get(session_id)
                .map(|p| p.has_minimal_output(5))
                .unwrap_or(true);
            if !is_minimal {
                continue;
            }
            let Some(session) = self.sessions.iter().find(|s| s.id == *session_id).cloned() else {
                continue;
            };
            self.providers.remove(session_id);
            self.last_pty_activity.remove(session_id);
            logger::info(&format!(
                "resume args exited without output for agent \"{}\", retrying with regular args",
                session.branch_name
            ));
            match self.spawn_pty_for_session(&session, false) {
                Ok(client) => {
                    self.providers.insert(session_id.clone(), client);
                    self.mark_session_status(session_id, SessionStatus::Active);
                    self.mark_session_provider_started(session_id);
                    let proj_name = self.project_name_for_session(&session);
                    self.set_info(format!(
                            "No prior session to resume for agent \"{}\". Started a fresh {} session in project \"{}\".",
                            session.branch_name,
                        session.provider.as_str(),
                        proj_name,
                    ));
                    retried.insert(session_id.clone());
                }
                Err(err) => {
                    logger::error(&format!(
                        "fallback PTY spawn also failed for {}: {err}",
                        session_id
                    ));
                    self.mark_session_status(session_id, SessionStatus::Detached);
                }
            }
        }

        for session_id in &exited {
            if retried.contains(session_id) {
                continue;
            }
            self.providers.remove(session_id);
            self.last_pty_activity.remove(session_id);
            self.mark_session_status(session_id, SessionStatus::Detached);
        }
        if !exited.is_empty() {
            // If the currently-viewed session just exited (and was not retried),
            // leave interactive mode.
            if let Some(current) = self.selected_session()
                && exited.contains(&current.id)
                && !retried.contains(&current.id)
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
            // Trigger PR status check for exited agents.
            for sid in &exited {
                if !retried.contains(sid) {
                    self.spawn_pr_check_for_session(sid);
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

        // Spawn a background worker to refresh resource monitor stats when
        // the overlay is open and enough wall-clock time has elapsed (~2s).
        if let PromptState::ResourceMonitor {
            ref last_refresh, ..
        } = self.prompt
            && last_refresh.elapsed() >= Duration::from_secs(2)
        {
            self.spawn_resource_stats_worker();
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

    pub(crate) fn spawn_branch_sync_worker(&self) {
        let interval_secs = self.config.ui.branch_sync_interval;
        if interval_secs == 0 {
            return; // disabled by config
        }
        let tx = self.worker_tx.clone();
        let sessions = Arc::clone(&self.branch_sync_sessions);
        thread::spawn(move || {
            let interval = Duration::from_secs(u64::from(interval_secs));
            loop {
                thread::sleep(interval);
                let snapshot = match sessions.lock() {
                    Ok(guard) => guard.clone(),
                    Err(_) => continue,
                };
                let mut updates = Vec::new();
                for entry in &snapshot {
                    if let Ok(actual) = git::current_branch(Path::new(&entry.worktree_path))
                        && actual != entry.branch_name
                    {
                        updates.push((entry.session_id.clone(), actual));
                    }
                }
                if !updates.is_empty() && tx.send(WorkerEvent::BranchSyncReady(updates)).is_err() {
                    break; // receiver dropped, app is shutting down
                }
            }
        });
    }

    // -- Git refs watcher for push detection --

    pub(crate) fn spawn_refs_watcher(&mut self) {
        use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};

        let tx = self.worker_tx.clone();
        // Build a reverse map of watched paths for event routing.
        let path_to_session: Arc<Mutex<HashMap<PathBuf, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let path_map = Arc::clone(&path_to_session);
        let debounce_map: Arc<Mutex<HashMap<String, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let debounce = Arc::clone(&debounce_map);

        let watcher_result = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else { return };
                // We only care about data modifications (ref file updates).
                if !event.kind.is_modify() && !event.kind.is_create() {
                    return;
                }
                let map = match path_map.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let mut debounce_guard = match debounce.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                for event_path in &event.paths {
                    // Walk up from the event path to find a watched parent dir.
                    for (watched, session_id) in map.iter() {
                        if event_path.starts_with(watched) {
                            // Debounce: skip if we already sent an event within the last 5s.
                            let now = Instant::now();
                            if let Some(last) = debounce_guard.get(session_id)
                                && now.duration_since(*last) < Duration::from_secs(5)
                            {
                                continue;
                            }
                            debounce_guard.insert(session_id.clone(), now);
                            logger::debug(&format!(
                                "[gh-integration] refs watcher: detected change at {}, debouncing for session {}",
                                event_path.display(),
                                session_id,
                            ));
                            let _ = tx.send(WorkerEvent::RefsChanged(session_id.clone()));
                        }
                    }
                }
            },
            NotifyConfig::default(),
        );

        match watcher_result {
            Ok(watcher) => {
                self.refs_watcher = Some(Arc::new(Mutex::new(watcher)));
                self.refs_watch_paths.clear();
                // Populate the path map and start watching existing sessions.
                let mut paths = HashMap::new();
                for session in &self.sessions {
                    let refs_dir = PathBuf::from(&session.worktree_path)
                        .join(".git")
                        .join("refs")
                        .join("heads");
                    if refs_dir.is_dir()
                        && let Some(ref watcher_arc) = self.refs_watcher
                        && let Ok(mut w) = watcher_arc.lock()
                    {
                        match w.watch(&refs_dir, RecursiveMode::NonRecursive) {
                            Ok(()) => {
                                logger::debug(&format!(
                                    "[gh-integration] refs watcher: watching {} for session {}",
                                    refs_dir.display(),
                                    session.id,
                                ));
                                paths.insert(refs_dir.clone(), session.id.clone());
                            }
                            Err(e) => {
                                logger::debug(&format!(
                                    "[gh-integration] refs watcher: failed to watch {}: {}",
                                    refs_dir.display(),
                                    e,
                                ));
                            }
                        }
                    }
                }
                self.refs_watch_paths = paths.clone();
                // Populate the closure's path map so events can route to sessions.
                if let Ok(mut map) = path_to_session.lock() {
                    *map = paths;
                }
                logger::info(&format!(
                    "[gh-integration] refs watcher: initialized, watching {} session(s)",
                    self.refs_watch_paths.len(),
                ));
            }
            Err(e) => {
                logger::warn(&format!(
                    "[gh-integration] refs watcher: failed to create watcher (falling back to poll-only): {}",
                    e,
                ));
            }
        }
    }

    // -- GitHub PR integration workers --

    pub(crate) fn spawn_gh_status_check(&self) {
        if !self.github_integration_enabled {
            return;
        }
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            use crate::model::GhStatus;
            // Step 1: Is `gh` on PATH?
            let on_path = std::process::Command::new("which")
                .arg("gh")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !on_path {
                logger::info("[gh-integration] gh CLI not found on PATH");
                let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::NotInstalled));
                return;
            }
            // Step 2: Is `gh` authenticated?
            let authed = std::process::Command::new("gh")
                .args(["auth", "status"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !authed {
                logger::info("[gh-integration] gh CLI found but not authenticated");
                let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::NotAuthenticated));
                return;
            }
            logger::info("[gh-integration] gh CLI available and authenticated");
            let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::Available));
        });
    }

    pub(crate) fn update_pr_sync_sessions(&self) {
        // Load known PRs from the database so the worker can use `gh pr view`
        // for sessions that already have a persisted PR association.
        let known_prs = self.session_store.load_all_latest_prs().unwrap_or_default();
        let known_map: HashMap<String, crate::storage::StoredPr> = known_prs
            .into_iter()
            .map(|pr| (pr.session_id.clone(), pr))
            .collect();

        if let Ok(mut guard) = self.pr_sync_sessions.lock() {
            *guard = self
                .sessions
                .iter()
                .map(|s| PrSyncEntry {
                    session_id: s.id.clone(),
                    branch_name: s.branch_name.clone(),
                    worktree_path: s.worktree_path.clone(),
                    known_pr: known_map.get(&s.id).cloned(),
                    agent_exited: !self.providers.contains_key(&s.id),
                })
                .collect();
        }
    }

    pub(crate) fn spawn_pr_sync_worker(&self) {
        let tx = self.worker_tx.clone();
        let sessions = Arc::clone(&self.pr_sync_sessions);
        let enabled = Arc::clone(&self.pr_sync_enabled);
        enabled.store(true, Ordering::Relaxed);
        thread::spawn(move || {
            let interval = Duration::from_secs(45);
            loop {
                thread::sleep(interval);
                if !enabled.load(Ordering::Relaxed) {
                    break;
                }
                let results = run_pr_sync(&sessions);
                if !results.is_empty() && tx.send(WorkerEvent::PrStatusReady(results)).is_err() {
                    break;
                }
            }
        });
    }

    pub(crate) fn spawn_initial_pr_refresh(&self) {
        let tx = self.worker_tx.clone();
        let sessions = Arc::clone(&self.pr_sync_sessions);
        thread::spawn(move || {
            let results = run_pr_sync(&sessions);
            if !results.is_empty() {
                let _ = tx.send(WorkerEvent::PrStatusReady(results));
            }
        });
    }

    /// Trigger a one-shot PR check for a single session, unless it was checked
    /// recently (within 10 seconds).
    pub(crate) fn spawn_pr_check_for_session(&mut self, session_id: &str) {
        if !self.github_integration_enabled
            || !matches!(self.gh_status, crate::model::GhStatus::Available)
        {
            return;
        }
        // Rate-limit: skip if checked within the last 10 seconds.
        if let Some(last) = self.pr_last_checked.get(session_id)
            && last.elapsed() < Duration::from_secs(10)
        {
            return;
        }
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            return;
        };
        let known_pr = self
            .session_store
            .load_prs(session_id)
            .ok()
            .and_then(|prs| prs.into_iter().next());
        let entry = PrSyncEntry {
            session_id: session.id.clone(),
            branch_name: session.branch_name.clone(),
            worktree_path: session.worktree_path.clone(),
            known_pr,
            agent_exited: !self.providers.contains_key(session_id),
        };
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            let result = check_pr_for_entry(&entry);
            let _ = tx.send(WorkerEvent::PrStatusReady(vec![(entry.session_id, result)]));
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

    fn retry_hung_resume_sessions(&mut self) {
        let mut hung = Vec::new();

        for (session_id, started_at) in &self.resume_fallback_candidates {
            let Some(session) = self.sessions.iter().find(|s| s.id == *session_id) else {
                continue;
            };
            let cfg = provider_config(&self.config, &session.provider);
            let Some(timeout_ms) = cfg.resume_wait_timeout_ms.filter(|timeout| *timeout > 0) else {
                continue;
            };
            if started_at.elapsed() < Duration::from_millis(timeout_ms) {
                continue;
            }
            let Some(provider) = self.providers.get(session_id) else {
                continue;
            };
            if provider.has_output() {
                continue;
            }
            hung.push(session_id.clone());
        }

        for session_id in hung {
            self.resume_fallback_candidates.remove(&session_id);
            let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
                continue;
            };
            self.providers.remove(&session_id);
            self.last_pty_activity.remove(&session_id);
            logger::info(&format!(
                "resume args produced no visible output for agent \"{}\" within timeout, retrying with regular args",
                session.branch_name
            ));
            match self.spawn_pty_for_session(&session, false) {
                Ok(client) => {
                    self.providers.insert(session_id.clone(), client);
                    self.mark_session_status(&session_id, SessionStatus::Active);
                    self.mark_session_provider_started(&session_id);
                    let proj_name = self.project_name_for_session(&session);
                    self.set_info(format!(
                        "Resume timed out for agent \"{}\" with no visible output. Started a fresh {} session in project \"{}\".",
                        session.branch_name,
                        session.provider.as_str(),
                        proj_name,
                    ));
                }
                Err(err) => {
                    logger::error(&format!(
                        "timeout fallback PTY spawn failed for {}: {err}",
                        session_id
                    ));
                    self.mark_session_status(&session_id, SessionStatus::Detached);
                }
            }
        }
    }
}

pub(crate) fn run_create_agent_job(
    request: CreateAgentRequest,
    paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
) {
    let (
        project,
        provider,
        source_branch,
        status_message,
        branch_name,
        worktree_path,
        owns_worktree,
    ) = match request {
        CreateAgentRequest::NewProject {
            project,
            custom_name,
            use_existing_branch,
        } => {
            let repo_path = PathBuf::from(&project.path);

            // Resolve the branch name early so we can check for an
            // existing branch before calling git worktree add.  When no
            // custom name was provided, a random pet name is generated.
            let resolved_name = custom_name.unwrap_or_else(git::docker_style_name);

            // If the caller already confirmed via the UI dialog,
            // `use_existing_branch` is true.  Otherwise, do a last-mile
            // check — this covers auto-generated pet names that
            // coincidentally match an existing branch.
            let attach_existing =
                use_existing_branch || git::branch_exists(&repo_path, &resolved_name).is_some();

            let progress = if attach_existing {
                format!(
                    "Attaching to existing branch \"{}\" for project \"{}\"...",
                    resolved_name, project.name
                )
            } else {
                format!(
                    "Creating a new worktree for project \"{}\"...",
                    project.name
                )
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(progress));

            let (branch_name, worktree_path) = if attach_existing {
                match git::create_worktree_existing_branch(
                    &repo_path,
                    &paths.worktrees_root,
                    &project.name,
                    &resolved_name,
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        logger::error(&format!(
                            "worktree creation (existing branch) failed for {}: {err}",
                            project.path
                        ));
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                            "Failed to attach to existing branch for project \"{}\": {err}",
                            project.name
                        )));
                        return;
                    }
                }
            } else {
                match git::create_worktree(
                    &repo_path,
                    &paths.worktrees_root,
                    &project.name,
                    Some(&resolved_name),
                ) {
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
                }
            };
            let status_message = if attach_existing {
                format!(
                    "Attached to existing branch \"{}\" in project \"{}\". The worktree is ready in a fresh session.",
                    branch_name, project.name
                )
            } else {
                format!(
                    "Created {} agent \"{}\" in project \"{}\". The new worktree is ready in a fresh session.",
                    project.default_provider.as_str(),
                    branch_name,
                    project.name
                )
            };
            (
                project.clone(),
                project.default_provider.clone(),
                project.current_branch.clone(),
                status_message,
                branch_name,
                worktree_path,
                true,
            )
        }
        CreateAgentRequest::ForkSession {
            project,
            source_session,
            source_label,
            custom_name,
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
                custom_name.as_deref(),
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
                true,
            )
        }
    };
    let repo_path = PathBuf::from(&project.path);
    if owns_worktree {
        logger::info(&format!(
            "created worktree {} on branch {}",
            worktree_path.display(),
            branch_name
        ));
    } else {
        logger::info(&format!(
            "reusing worktree {} on branch {} for new provider session",
            worktree_path.display(),
            branch_name
        ));
    }
    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
        provider,
        source_branch,
        branch_name,
        worktree_path: worktree_path.to_string_lossy().to_string(),
        title: None,
        started_providers: Vec::new(),
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let provider_cfg = provider_config(&config, &session.provider);
    if let Err(hint) = check_provider_available(&provider_cfg) {
        logger::error(&format!("provider not found for {}: {hint}", session.id));
        if owns_worktree {
            let _ = git::remove_worktree(
                &repo_path,
                Path::new(&session.worktree_path),
                &session.branch_name,
            );
        }
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
            if owns_worktree {
                let _ = git::remove_worktree(
                    &repo_path,
                    Path::new(&session.worktree_path),
                    &session.branch_name,
                );
            }
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

// -- GitHub PR sync helpers (run on background threads) --

fn run_pr_sync(
    sessions: &Arc<Mutex<Vec<PrSyncEntry>>>,
) -> Vec<(String, Option<crate::model::PrInfo>)> {
    let snapshot = match sessions.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return Vec::new(),
    };
    snapshot
        .iter()
        .map(|entry| {
            let result = check_pr_for_entry(entry);
            (entry.session_id.clone(), result)
        })
        .collect()
}

/// Determine the current PR state for a session. The check strategy depends on
/// what we already know and whether the agent is still running:
///
/// | Known PR state | Agent running? | Action                                |
/// |----------------|---------------|---------------------------------------|
/// | None           | any           | `gh pr list --head` to discover       |
/// | OPEN           | any           | `gh pr view` + discover newer         |
/// | MERGED/CLOSED  | yes           | discover newer (agent may push again) |
/// | MERGED/CLOSED  | no            | **zero calls** — nothing will change  |
///
/// The last row is the key optimization: once a PR is in a terminal state and
/// the agent has exited, nobody is pushing to that branch anymore, so there is
/// no reason to check for newer PRs. This reduces API calls from O(sessions)
/// to O(active_sessions) for repos with many completed agents.
fn check_pr_for_entry(entry: &PrSyncEntry) -> Option<crate::model::PrInfo> {
    let owner_repo = git::remote_owner_repo(Path::new(&entry.worktree_path))
        .or_else(|| entry.known_pr.as_ref().map(|pr| pr.owner_repo.clone()))?;

    if let Some(ref known) = entry.known_pr {
        let is_terminal = known.state == "MERGED" || known.state == "CLOSED";

        if is_terminal {
            if entry.agent_exited {
                // Terminal PR + exited agent = zero network calls.
                // The agent process is gone and the PR is already merged/closed,
                // so no new commits or PRs will appear on this branch.
                return reconstruct_from_stored(known);
            }

            // Terminal PR but agent is still running — it might push new commits
            // and open a follow-up PR, so we still check for newer PRs.
            if let Some(newer) =
                discover_pr_by_branch(&entry.branch_name, &owner_repo, &entry.session_id)
                && newer.number > known.pr_number
            {
                return Some(newer);
            }
            return reconstruct_from_stored(known);
        }

        // Open PR: refresh its current state via `gh pr view`.
        if let Some(pr) = view_pr_by_number(known.pr_number, &known.owner_repo, &entry.session_id) {
            // Also check if a newer PR was opened.
            if let Some(newer) =
                discover_pr_by_branch(&entry.branch_name, &owner_repo, &entry.session_id)
                && newer.number > pr.number
            {
                return Some(newer);
            }
            return Some(pr);
        }
    }

    // No known PR — discover by branch name.
    discover_pr_by_branch(&entry.branch_name, &owner_repo, &entry.session_id)
}

/// Reconstruct a PrInfo from stored data without a network call.
/// Used for terminal states (merged/closed) that don't need refreshing.
fn reconstruct_from_stored(stored: &crate::storage::StoredPr) -> Option<crate::model::PrInfo> {
    use crate::model::{PrInfo, PrState};
    let state = match stored.state.as_str() {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        "OPEN" => PrState::Open,
        _ => return None,
    };
    Some(PrInfo {
        number: stored.pr_number,
        state,
        title: stored.title.clone(),
        owner_repo: stored.owner_repo.clone(),
    })
}

/// Check a known PR by number using `gh pr view`.
fn view_pr_by_number(
    number: u64,
    owner_repo: &str,
    session_id: &str,
) -> Option<crate::model::PrInfo> {
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            owner_repo,
            "--json",
            "number,state,title",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        logger::debug(&format!(
            "[gh-integration] gh pr view #{number} failed for {session_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_pr_json_object(text.trim(), owner_repo)
}

/// Discover a PR by branch name using `gh pr list --state all`.
fn discover_pr_by_branch(
    branch: &str,
    owner_repo: &str,
    session_id: &str,
) -> Option<crate::model::PrInfo> {
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--repo",
            owner_repo,
            "--state",
            "all",
            "--json",
            "number,state,title",
            "--limit",
            "1",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        logger::debug(&format!(
            "[gh-integration] gh pr list failed for {session_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let arr: Vec<serde_json::Value> = serde_json::from_str(text.trim()).ok()?;
    let obj = arr.first()?;
    parse_pr_json_value(obj, owner_repo)
}

/// Parse a single PR JSON object (from `gh pr view` output).
fn parse_pr_json_object(json: &str, owner_repo: &str) -> Option<crate::model::PrInfo> {
    let obj: serde_json::Value = serde_json::from_str(json).ok()?;
    parse_pr_json_value(&obj, owner_repo)
}

/// Extract PrInfo from a serde_json::Value.
fn parse_pr_json_value(obj: &serde_json::Value, owner_repo: &str) -> Option<crate::model::PrInfo> {
    use crate::model::{PrInfo, PrState};

    let number = obj.get("number")?.as_u64()?;
    let state_str = obj.get("state")?.as_str()?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let state = match state_str {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => return None,
    };

    Some(PrInfo {
        number,
        state,
        title,
        owner_repo: owner_repo.to_string(),
    })
}
