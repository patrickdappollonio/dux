use super::*;

impl App {
    pub(crate) fn drain_events(&mut self) {
        while let Ok(event) = self.worker_rx.try_recv() {
            match event {
                WorkerEvent::CreateAgentProgress(message) => self.set_busy(message),
                WorkerEvent::CreateAgentFailed(message) => {
                    self.create_agent_in_flight = false;
                    self.set_error(message);
                }
                WorkerEvent::AgentLaunchReady(boxed) => {
                    self.handle_agent_launch_ready(*boxed);
                }
                WorkerEvent::AgentLaunchFailed(boxed) => {
                    self.handle_agent_launch_failed(*boxed);
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
                                    let warning = branch_warning_kind(
                                        Path::new(&existing.path),
                                        &existing.current_branch,
                                    );
                                    existing.branch_status = branch_status_from_warning(warning.as_ref());
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
                                let _ = self.session_store.upsert_pr(&crate::storage::StoredPr {
                                    session_id: session_id.clone(),
                                    pr_number: pr.number,
                                    host: pr.host.clone(),
                                    owner_repo: pr.owner_repo.clone(),
                                    state: state_str.to_string(),
                                    title: pr.title.clone(),
                                    url: pr.url.clone(),
                                });
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
                WorkerEvent::PullRequestResolved { result } => match result {
                    Ok(pr) => {
                        let request = CreateAgentRequest::PullRequest {
                            project: pr.project.clone(),
                            host: pr.host.clone(),
                            owner_repo: pr.owner_repo.clone(),
                            number: pr.number,
                            title: pr.title.clone(),
                            state: pr.state.clone(),
                            head_branch: pr.head_ref_name.clone(),
                            custom_name: Some(pr.head_ref_name.clone()),
                            use_existing_branch: false,
                        };
                        if let Err(err) = self.open_name_new_agent_prompt(request) {
                            self.set_error(format!("{err:#}"));
                        } else {
                            self.set_info(format!(
                                "Resolved PR #{}: {}. Confirm or edit the branch name.",
                                pr.number, pr.title
                            ));
                        }
                    }
                    Err(message) => {
                        self.set_error(message);
                    }
                },
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
                WorkerEvent::ProjectWorktreesReady { project_id, result } => {
                    let mut status_after_update: Option<Result<&'static str, String>> = None;
                    if let PromptState::PickProjectWorktree(prompt) = &mut self.prompt
                        && prompt.project.id == project_id
                    {
                        prompt.loading = false;
                        match result {
                            Ok(entries) => {
                                prompt.selected =
                                    selectable_project_worktree_indices(&entries).into_iter().next();
                                prompt.entries = entries;
                                prompt.error = None;
                                status_after_update = Some(Ok(
                                    "Choose an available worktree to launch a new agent.",
                                ));
                            }
                            Err(error) => {
                                let project_name = prompt.project.name.clone();
                                prompt.entries.clear();
                                prompt.selected = None;
                                prompt.error = Some(error.clone());
                                status_after_update = Some(Err(format!(
                                    "Failed to load worktrees for project \"{}\": {error}",
                                    project_name
                                )));
                            }
                        }
                    }
                    if let Some(status) = status_after_update {
                        match status {
                            Ok(message) => self.set_info(message),
                            Err(message) => self.set_error(message),
                        }
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
                WorkerEvent::NonDefaultBranchCheckoutCompleted {
                    action,
                    target_branch,
                    result,
                } => match result {
                    Ok(()) => match action {
                        NonDefaultBranchAction::AddProject {
                            path,
                            name,
                            leading_branch,
                        } => {
                            let display_name = if name.trim().is_empty() {
                                std::path::Path::new(&path)
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("project")
                                    .to_string()
                            } else {
                                name.trim().to_string()
                            };
                            let status_message = format!(
                                "Checked out \"{target_branch}\" and added project \"{display_name}\" to workspace."
                            );
                            if let Err(e) = self.finish_add_project_with_status(
                                path,
                                name,
                                target_branch.clone(),
                                leading_branch,
                                status_message,
                            ) {
                                self.set_error(format!("{e:#}"));
                            }
                        }
                        NonDefaultBranchAction::CheckoutProjectDefault { project } => {
                            if let Some(existing) =
                                self.projects.iter_mut().find(|p| p.id == project.id)
                            {
                                existing.current_branch = target_branch.clone();
                                existing.branch_status = ProjectBranchStatus::Leading;
                            }
                            self.set_info(format!(
                                "Checked out \"{target_branch}\" for project \"{}\".",
                                project.name
                            ));
                        }
                    },
                    Err(err) => {
                        // Preserve the full git stderr in the log so
                        // debugging stays possible after the status line
                        // summary is overwritten by the next message.
                        let path = action.repo_path().to_string();
                        logger::error(&format!(
                            "non-default branch checkout failed for {path}: {err}"
                        ));
                        self.set_error(format!(
                            "Couldn't check out \"{target_branch}\" in {path} — resolve in your terminal and retry."
                        ));
                    }
                },
                WorkerEvent::CreateAgentBranchInspected { project, result } => match result {
                    Ok(inspection) => {
                        if let Some(existing) =
                            self.projects.iter_mut().find(|p| p.id == project.id)
                        {
                            existing.current_branch = inspection.current_branch.clone();
                            existing.leading_branch = Some(inspection.leading_branch.clone());
                            existing.branch_status =
                                if existing.current_branch == inspection.leading_branch {
                                    ProjectBranchStatus::Leading
                                } else {
                                    ProjectBranchStatus::NotLeading
                                };
                        }
                        if let Err(err) =
                            self.continue_create_agent_after_branch_inspection(project, inspection)
                        {
                            self.set_error(format!("{err:#}"));
                        }
                    }
                    Err(err) => {
                        self.set_error(err);
                    }
                },
                WorkerEvent::ProjectBranchStatusReady { project_id, result } => match result {
                    Ok((current_branch, branch_status)) => {
                        if let Some(project) =
                            self.projects.iter_mut().find(|p| p.id == project_id)
                        {
                            project.current_branch = current_branch;
                            project.branch_status = branch_status;
                        }
                    }
                    Err(err) => {
                        logger::debug(&format!(
                            "project branch status inspection failed for {project_id}: {err}"
                        ));
                    }
                },
                WorkerEvent::CheckoutProjectDefaultBranchInspected { project, result } => {
                    match result {
                        Ok((current_branch, warning_kind)) => match warning_kind {
                            Some(BranchWarningKind::Known { default_branch }) => {
                                let mut project = project;
                                project.current_branch = current_branch;
                                self.dispatch_non_default_branch_checkout(
                                    NonDefaultBranchAction::CheckoutProjectDefault { project },
                                    default_branch,
                                    "for the selected project".to_string(),
                                );
                            }
                            Some(BranchWarningKind::Heuristic) => {
                                self.set_error(format!(
                                    "Can't determine the default branch for project \"{}\" while it is on \"{}\". Resolve the default branch in your terminal and retry.",
                                    project.name, current_branch
                                ));
                            }
                            None => {
                                if let Some(existing) =
                                    self.projects.iter_mut().find(|p| p.id == project.id)
                                {
                                    existing.current_branch = current_branch.clone();
                                    existing.branch_status = ProjectBranchStatus::Leading;
                                }
                                self.set_info(format!(
                                    "Project \"{}\" is already on the leading branch \"{}\".",
                                    project.name, current_branch
                                ));
                            }
                        },
                        Err(err) => self.set_error(format!(
                            "Couldn't inspect the default branch for project \"{}\": {err}",
                            project.name
                        )),
                    }
                },
                WorkerEvent::ConfigReloadReady(result) => match *result {
                    Ok(config) => {
                        if let Err(err) = self.apply_reloaded_config(config) {
                            self.set_error(format!(
                                "Config validation passed, but applying it failed: {err:#}"
                            ));
                        } else {
                            self.set_info(
                                "Configuration reloaded. New settings are active now.",
                            );
                        }
                    }
                    Err(message) => {
                        self.open_config_reload_failed_modal(message);
                        self.set_error(
                            "Config reload failed. Review the modal before retrying.",
                        );
                    }
                },
                WorkerEvent::ConfigRecoverCompleted(result) => match result {
                    Ok(()) => {
                        self.set_info(
                            "Restored the last working configuration to config.toml.",
                        );
                    }
                    Err(message) => {
                        self.set_error(format!(
                            "Couldn't restore the last working configuration: {message}"
                        ));
                    }
                },
                WorkerEvent::ProjectPersistenceCompleted { action, result } => {
                    self.apply_project_persistence_result(action, result);
                }
            }
        }
        self.retry_hung_resume_sessions();
        // Detect PTY exits.
        let mut exited = Vec::new();
        for (session_id, provider) in &mut self.providers {
            let exit_success = provider.try_wait().map(|status| status.success());
            if exit_success.is_some() || provider.is_exited() {
                exited.push((session_id.clone(), exit_success));
            }
        }

        // For sessions that were spawned with resume_args and exited before
        // producing any output, retry with regular args (fresh session).
        // This handles `claude --continue || claude` style fallback.
        let mut retried = HashSet::new();
        for (session_id, _) in &exited {
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
            self.running_provider_pins.remove(session_id);
            self.last_pty_activity.remove(session_id);
            logger::info(&format!(
                "resume args exited without output for agent \"{}\", retrying with regular args",
                session.branch_name
            ));
            let proj_name = self.project_name_for_session(&session);
            let status_message = format!(
                "No prior session to resume for agent \"{}\". Started a fresh {} session in project \"{}\".",
                session.branch_name,
                session.provider.as_str(),
                proj_name,
            );
            let request = self.agent_launch_request(
                session,
                false,
                AgentLaunchKind::ResumeFallback { status_message },
            );
            if self.dispatch_agent_launch(request) {
                retried.insert(session_id.clone());
            } else {
                self.mark_session_status(session_id, SessionStatus::Detached);
            }
        }

        for (session_id, exit_success) in &exited {
            if retried.contains(session_id) {
                continue;
            }
            self.providers.remove(session_id);
            self.running_provider_pins.remove(session_id);
            self.last_pty_activity.remove(session_id);
            if *exit_success == Some(true) {
                self.mark_session_desired_running(session_id, false);
            }
            self.mark_session_status(session_id, SessionStatus::Detached);
        }
        if !exited.is_empty() {
            // If the currently-viewed session just exited (and was not retried),
            // leave interactive mode.
            if let Some(current) = self.selected_session()
                && exited.iter().any(|(id, _)| id == &current.id)
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
                if !retried.contains(&sid.0) {
                    self.spawn_pr_check_for_session(&sid.0);
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

    fn apply_project_persistence_result(
        &mut self,
        action: ProjectPersistenceAction,
        result: Result<(), String>,
    ) {
        if let Err(err) = result {
            match action {
                ProjectPersistenceAction::Add { project, .. } => {
                    self.set_error(format!(
                        "Could not save project \"{}\" to the database: {err}",
                        project.name
                    ));
                }
                ProjectPersistenceAction::Remove { project_name, .. } => {
                    self.set_error(format!(
                        "Could not remove project \"{project_name}\" from the database: {err}"
                    ));
                }
                ProjectPersistenceAction::Delete { project_name, .. } => {
                    self.set_error(format!(
                        "Could not finish deleting project \"{project_name}\" from the database: {err}"
                    ));
                }
                ProjectPersistenceAction::UpdateDefaultProvider { project_name, .. } => {
                    self.set_error(format!(
                        "Could not save the provider change for project \"{project_name}\": {err}"
                    ));
                }
                ProjectPersistenceAction::UpdateAutoReopen { project_name, .. } => {
                    self.set_error(format!(
                        "Could not save the auto-reopen change for project \"{project_name}\": {err}"
                    ));
                }
            }
            return;
        }

        match action {
            ProjectPersistenceAction::Add {
                project,
                status_message,
            } => {
                let project_id = project.id.clone();
                self.projects.push(project);
                self.rebuild_left_items();
                if let Some(index) = self.left_items().iter().position(|item| {
                    matches!(item, LeftItem::Project(project_index) if self.projects[*project_index].id == project_id)
                }) {
                    self.selected_left = index;
                }
                self.set_info(status_message);
            }
            ProjectPersistenceAction::Remove {
                project_id,
                project_name,
            } => {
                self.projects.retain(|project| project.id != project_id);
                self.rebuild_left_items();
                self.selected_left = self.selected_left.saturating_sub(1);
                self.set_info(format!("Removed project \"{project_name}\" from app"));
            }
            ProjectPersistenceAction::Delete {
                project_id,
                project_name,
            } => {
                self.projects.retain(|project| project.id != project_id);
                self.rebuild_left_items();
                self.selected_left = self.selected_left.saturating_sub(1);
                self.reload_changed_files();
                self.set_info(format!(
                    "Deleted project \"{project_name}\" and all its agents"
                ));
            }
            ProjectPersistenceAction::UpdateDefaultProvider {
                project_id,
                project_name,
                provider,
                global_default,
            } => {
                if let Some(project) = self
                    .projects
                    .iter_mut()
                    .find(|project| project.id == project_id)
                {
                    project.explicit_default_provider = provider.clone();
                }
                refresh_project_defaults(&mut self.projects, &self.config);
                self.rebuild_left_items();
                let message = match provider {
                    Some(provider) => format!(
                        "Project provider for \"{}\" changed to {}. Future agents in this project will use it; existing agents keep their current provider.",
                        project_name,
                        provider.as_str(),
                    ),
                    None => format!(
                        "\"{}\" now inherits the global default provider ({}). Future agents in this project will use it; existing agents keep their current provider.",
                        project_name,
                        global_default.as_str(),
                    ),
                };
                self.set_info(message);
            }
            ProjectPersistenceAction::UpdateAutoReopen {
                project_id,
                project_name,
                auto_reopen_agents,
            } => {
                if let Some(project) = self
                    .projects
                    .iter_mut()
                    .find(|project| project.id == project_id)
                {
                    project.auto_reopen_agents = auto_reopen_agents;
                }
                let enabled = auto_reopen_agents.unwrap_or(true);
                self.set_info(format!(
                    "Startup auto-reopen {} for project \"{}\".",
                    if enabled { "enabled" } else { "disabled" },
                    project_name
                ));
            }
        }
    }

    pub(crate) fn spawn_project_persistence(&self, action: ProjectPersistenceAction) {
        let db_path = self.paths.sessions_db_path.clone();
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let store = SessionStore::open(&db_path)?;
                match &action {
                    ProjectPersistenceAction::Add { project, .. } => {
                        store.upsert_project(&crate::config::ProjectConfig {
                            id: project.id.clone(),
                            path: project.path.clone(),
                            name: Some(project.name.clone()),
                            default_provider: project
                                .explicit_default_provider
                                .as_ref()
                                .map(|provider| provider.as_str().to_string()),
                            leading_branch: project.leading_branch.clone(),
                            auto_reopen_agents: project.auto_reopen_agents,
                        })?;
                    }
                    ProjectPersistenceAction::Remove { project_id, .. }
                    | ProjectPersistenceAction::Delete { project_id, .. } => {
                        store.delete_project(project_id)?;
                    }
                    ProjectPersistenceAction::UpdateDefaultProvider {
                        project_id,
                        provider,
                        ..
                    } => {
                        store.update_project_default_provider(
                            project_id,
                            provider.as_ref().map(|provider| provider.as_str()),
                        )?;
                    }
                    ProjectPersistenceAction::UpdateAutoReopen {
                        project_id,
                        auto_reopen_agents,
                        ..
                    } => {
                        store.update_project_auto_reopen(project_id, *auto_reopen_agents)?;
                    }
                }
                Ok(())
            })()
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::ProjectPersistenceCompleted { action, result });
        });
    }

    fn handle_agent_launch_ready(&mut self, data: AgentLaunchReadyData) {
        let AgentLaunchReadyData { request, client } = data;
        let session = request.session.clone();
        let session_id = session.id.clone();
        self.agent_launches_in_flight.remove(&session_id);
        self.last_pty_size = request.pty_size;

        if matches!(request.kind, AgentLaunchKind::Create { .. }) {
            self.create_agent_in_flight = false;
            if let Err(err) = self.session_store.upsert_session(&session) {
                logger::error(&format!(
                    "session store upsert failed for {}: {err}",
                    session.id
                ));
                self.set_error(format!("Failed to persist session: {err}"));
                return;
            }
            self.detach_conflicting_worktree_session(&session.worktree_path, &session.id);
            self.providers.insert(session.id.clone(), client);
            self.sessions.insert(0, session.clone());
            self.mark_session_provider_started(&session.id);
            if request.resume {
                self.resume_fallback_candidates
                    .insert(session.id.clone(), Instant::now());
            }
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
            if let AgentLaunchKind::Create { status_message, .. } = request.kind {
                self.set_info(status_message);
            }
            return;
        }

        if !self.sessions.iter().any(|s| s.id == session.id) {
            logger::info(&format!(
                "dropping launched PTY for missing session {}",
                session.id
            ));
            return;
        }

        self.detach_conflicting_worktree_session(&session.worktree_path, &session.id);
        self.providers.insert(session.id.clone(), client);
        if request.resume {
            self.resume_fallback_candidates
                .insert(session.id.clone(), Instant::now());
        }
        self.mark_session_desired_running(&session.id, true);
        self.mark_session_status(&session.id, SessionStatus::Active);
        self.mark_session_provider_started(&session.id);

        match request.kind {
            AgentLaunchKind::Reconnect { status_message }
            | AgentLaunchKind::ForceReconnect { status_message } => {
                self.show_agent_surface();
                self.input_target = InputTarget::Agent;
                self.fullscreen_overlay = FullscreenOverlay::Agent;
                self.set_info(status_message);
            }
            AgentLaunchKind::ResumeFallback { status_message } => {
                self.set_info(status_message);
            }
            AgentLaunchKind::StartupAutoReopen => {}
            AgentLaunchKind::Create { .. } => unreachable!("create launch handled above"),
        }
    }

    fn handle_agent_launch_failed(&mut self, data: AgentLaunchFailedData) {
        let AgentLaunchFailedData { request, message } = data;
        let session = request.session;
        self.agent_launches_in_flight.remove(&session.id);

        match request.kind {
            AgentLaunchKind::Create { .. } => {
                self.create_agent_in_flight = false;
                self.set_error(message);
            }
            AgentLaunchKind::Reconnect { .. } => {
                self.set_error(format!(
                    "Reconnect failed for agent \"{}\": {}",
                    session.branch_name, message
                ));
            }
            AgentLaunchKind::ForceReconnect { .. } => {
                self.set_error(format!(
                    "Fresh restart failed for agent \"{}\": {}",
                    session.branch_name, message
                ));
            }
            AgentLaunchKind::ResumeFallback { .. } => {
                logger::error(&format!(
                    "fallback PTY spawn failed for {}: {}",
                    session.id, message
                ));
                self.mark_session_status(&session.id, SessionStatus::Detached);
            }
            AgentLaunchKind::StartupAutoReopen => {
                logger::error(&format!(
                    "startup auto-reopen failed for agent \"{}\": {}",
                    session.branch_name, message
                ));
                self.set_warning(format!(
                    "Couldn't auto-reopen agent \"{}\": {}",
                    session.branch_name, message
                ));
            }
        }
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

    pub(crate) fn spawn_project_worktrees_worker(&self, project: Project) {
        let tx = self.worker_tx.clone();
        let paths = self.paths.clone();
        let sessions = self.sessions.clone();
        thread::spawn(move || {
            let result = git::list_worktrees(Path::new(&project.path))
                .map(|worktrees| classify_project_worktrees(&project, &paths, &sessions, worktrees))
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::ProjectWorktreesReady {
                project_id: project.id,
                result,
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

    pub(crate) fn spawn_project_branch_status_checks(&self) {
        for project in self.projects.iter().filter(|project| !project.path_missing) {
            let project = project.clone();
            let worker_tx = self.worker_tx.clone();
            thread::spawn(move || {
                run_project_branch_status_job(project, worker_tx);
            });
        }
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

    pub(crate) fn spawn_config_reload_worker(&self) {
        let tx = self.worker_tx.clone();
        let paths = self.paths.clone();
        thread::spawn(move || {
            let result = crate::config::ensure_config(&paths)
                .map_err(|err| format!("{err:#}"))
                .and_then(|config| match crate::config::validate_keys(&config.keys) {
                    Ok(()) => Ok(config),
                    Err(message) => Err(message),
                });
            let _ = tx.send(WorkerEvent::ConfigReloadReady(Box::new(result)));
        });
    }

    pub(crate) fn spawn_config_recover_worker(&self) {
        let tx = self.worker_tx.clone();
        let config_path = self.paths.config_path.clone();
        let config = self.config.clone();
        thread::spawn(move || {
            let bindings = RuntimeBindings::from_keys_config(&config.keys);
            let rendered = crate::config::render_config_with(&config, &bindings);
            let result = std::fs::write(&config_path, rendered)
                .map_err(|err| format!("failed to write {}: {err}", config_path.display()));
            let _ = tx.send(WorkerEvent::ConfigRecoverCompleted(result));
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
            self.running_provider_pins.remove(&session_id);
            self.last_pty_activity.remove(&session_id);
            logger::info(&format!(
                "resume args produced no visible output for agent \"{}\" within timeout, retrying with regular args",
                session.branch_name
            ));
            let proj_name = self.project_name_for_session(&session);
            let status_message = format!(
                "Resume timed out for agent \"{}\" with no visible output. Started a fresh {} session in project \"{}\".",
                session.branch_name,
                session.provider.as_str(),
                proj_name,
            );
            let request = self.agent_launch_request(
                session,
                false,
                AgentLaunchKind::ResumeFallback { status_message },
            );
            if !self.dispatch_agent_launch(request) {
                self.mark_session_status(&session_id, SessionStatus::Detached);
            }
        }
    }
}

/// Background job for "Add Project" when the user opted to have dux switch to
/// the default branch first. Runs `git switch <target_branch>` in the source
/// repo and reports the outcome via
/// `WorkerEvent::NonDefaultBranchCheckoutCompleted` so the main loop can
/// continue the selected action or surface the error.
pub(crate) fn run_add_project_checkout_job(
    action: NonDefaultBranchAction,
    target_branch: String,
    worker_tx: Sender<WorkerEvent>,
) {
    let path = action.repo_path().to_string();
    let result = git::switch_branch(Path::new(&path), &target_branch).map_err(|e| format!("{e:#}"));
    let _ = worker_tx.send(WorkerEvent::NonDefaultBranchCheckoutCompleted {
        action,
        target_branch,
        result,
    });
}

pub(crate) fn run_create_agent_branch_inspection_job(
    project: Project,
    worker_tx: Sender<WorkerEvent>,
) {
    let repo_path = PathBuf::from(&project.path);
    let result = git::current_branch(&repo_path)
        .map_err(|err| {
            format!(
                "Couldn't inspect the current branch for project \"{}\": {err:#}",
                project.name
            )
        })
        .and_then(|current_branch| {
            let leading_branch = project
                .leading_branch
                .clone()
                .unwrap_or_else(|| leading_branch_for_project(&repo_path, &current_branch));
            if !git::local_branch_exists(&repo_path, &leading_branch) {
                return Err(format!(
                    "Cannot create agent for \"{}\": leading branch \"{}\" no longer exists locally. Restore that branch or re-add the project.",
                    project.name, leading_branch
                ));
            }
            Ok(CreateAgentBranchInspection {
                current_branch,
                leading_branch,
            })
        });
    let _ = worker_tx.send(WorkerEvent::CreateAgentBranchInspected { project, result });
}

pub(crate) fn run_project_branch_status_job(project: Project, worker_tx: Sender<WorkerEvent>) {
    let repo_path = PathBuf::from(&project.path);
    let result = git::current_branch(&repo_path)
        .map(|branch| {
            let branch_status = if let Some(leading_branch) = project.leading_branch.as_deref() {
                if branch == leading_branch {
                    ProjectBranchStatus::Leading
                } else {
                    ProjectBranchStatus::NotLeading
                }
            } else {
                let warning_kind = branch_warning_kind(&repo_path, &branch);
                branch_status_from_warning(warning_kind.as_ref())
            };
            (branch, branch_status)
        })
        .map_err(|err| format!("{err:#}"));
    let _ = worker_tx.send(WorkerEvent::ProjectBranchStatusReady {
        project_id: project.id,
        result,
    });
}

pub(crate) fn run_checkout_project_default_branch_inspection_job(
    project: Project,
    worker_tx: Sender<WorkerEvent>,
) {
    let repo_path = PathBuf::from(&project.path);
    let result = git::current_branch(&repo_path)
        .map(|branch| {
            let warning_kind = branch_warning_kind(&repo_path, &branch);
            (branch, warning_kind)
        })
        .map_err(|err| format!("{err:#}"));
    let _ = worker_tx.send(WorkerEvent::CheckoutProjectDefaultBranchInspected { project, result });
}

pub(crate) fn run_pull_request_lookup_job(
    project: Project,
    raw_input: String,
    worker_tx: Sender<WorkerEvent>,
) {
    let lookup = match git::remote_github_repo(Path::new(&project.path)) {
        Some(remote) => {
            super::sessions::parse_pull_request_lookup(&raw_input, &remote.host, &remote.owner_repo)
        }
        None => Err(format!(
            "Project \"{}\" does not have a GitHub origin remote.",
            project.name
        )),
    };
    let lookup = match lookup {
        Ok(lookup) => lookup,
        Err(message) => {
            let _ = worker_tx.send(WorkerEvent::PullRequestResolved {
                result: Err(message),
            });
            return;
        }
    };

    let repo = gh_repo_arg(&lookup.host, &lookup.owner_repo);
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            &lookup.number.to_string(),
            "--repo",
            &repo,
            "--json",
            "number,title,state,headRefName",
        ])
        .output();
    let result = match output {
        Ok(output) if output.status.success() => parse_resolved_pull_request_json(
            &String::from_utf8_lossy(&output.stdout),
            project,
            &lookup.host,
            &lookup.owner_repo,
        ),
        Ok(output) => Err(format!(
            "Failed to resolve PR #{} from {}: {}",
            lookup.number,
            lookup.owner_repo,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(err) => Err(format!("Failed to run gh pr view: {err}")),
    };
    let _ = worker_tx.send(WorkerEvent::PullRequestResolved { result });
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
        title,
        launch_with_resume,
    ) = match request {
        CreateAgentRequest::NewProject {
            project,
            custom_name,
            use_existing_branch,
            pull_before_create,
        } => {
            let repo_path = PathBuf::from(&project.path);

            if pull_before_create {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                    "Pulling latest changes for project \"{}\" before creating the agent...",
                    project.name
                )));
                if let Err(err) = git::pull_current_branch(&repo_path) {
                    logger::error(&format!(
                        "pre-create pull failed for {}: {err}",
                        project.path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to pull latest changes for project \"{}\" before creating the agent: {err}",
                        project.name
                    )));
                    return;
                }
            }

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
            let leading_branch = project
                .leading_branch
                .clone()
                .unwrap_or_else(|| project.current_branch.clone());
            if !attach_existing && !git::local_branch_exists(&repo_path, &leading_branch) {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Cannot create agent for \"{}\": leading branch \"{}\" no longer exists locally. Restore that branch or re-add the project.",
                    project.name, leading_branch
                )));
                return;
            }

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
                match git::create_worktree_from_start_point(
                    &repo_path,
                    &paths.worktrees_root,
                    &project.name,
                    Some(&leading_branch),
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
                if attach_existing {
                    project.current_branch.clone()
                } else {
                    leading_branch
                },
                status_message,
                branch_name,
                worktree_path,
                true,
                None,
                false,
            )
        }
        CreateAgentRequest::PullRequest {
            project,
            host,
            owner_repo,
            number,
            title,
            state,
            head_branch,
            custom_name,
            use_existing_branch,
        } => {
            let repo_path = PathBuf::from(&project.path);
            let resolved_name = custom_name.unwrap_or_else(|| head_branch.clone());
            let attach_existing =
                use_existing_branch || git::branch_exists(&repo_path, &resolved_name).is_some();

            if attach_existing {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                    "Attaching to existing branch \"{}\" for PR #{} in project \"{}\"...",
                    resolved_name, number, project.name
                )));
            } else {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                    "Fetching PR #{} from {} into branch \"{}\"...",
                    number, owner_repo, resolved_name
                )));
                if let Err(err) = git::fetch_pull_request_head(&repo_path, number, &resolved_name) {
                    logger::error(&format!(
                        "PR worktree fetch failed for {} #{}: {err}",
                        owner_repo, number
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to fetch PR #{} from {}: {err}",
                        number, owner_repo
                    )));
                    return;
                }
            }

            let (branch_name, worktree_path) = match git::create_worktree_existing_branch(
                &repo_path,
                &paths.worktrees_root,
                &project.name,
                &resolved_name,
            ) {
                Ok(result) => result,
                Err(err) => {
                    logger::error(&format!(
                        "PR worktree creation failed for {} #{}: {err}",
                        owner_repo, number
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to create a worktree for PR #{} in project \"{}\": {err}",
                        number, project.name
                    )));
                    return;
                }
            };
            let status_message = format!(
                "Created {} agent \"{}\" from PR #{} ({}) in project \"{}\".",
                project.default_provider.as_str(),
                branch_name,
                number,
                title,
                project.name
            );
            logger::info(&format!(
                "created PR worktree from {} #{} ({state}) {}",
                owner_repo,
                number,
                pull_request_url(&host, &owner_repo, number)
            ));
            (
                project.clone(),
                project.default_provider.clone(),
                project.current_branch.clone(),
                status_message,
                branch_name,
                worktree_path,
                true,
                None,
                false,
            )
        }
        CreateAgentRequest::ForkSession {
            project,
            source_session,
            source_label,
            custom_name,
        } => {
            let Some(custom_name) = custom_name else {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(
                    "Forking an agent requires choosing a name first.".to_string(),
                ));
                return;
            };
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
                Some(&custom_name),
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
                None,
                false,
            )
        }
        CreateAgentRequest::ExistingManagedWorktree {
            project,
            worktree_path,
            branch_name,
            custom_name,
        } => {
            let agent_name = custom_name.clone().unwrap_or_else(|| branch_name.clone());
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Launching {} in existing worktree \"{}\"...",
                project.default_provider.as_str(),
                worktree_path.display(),
            )));
            let status_message = format!(
                "Imported {} agent \"{}\" from existing managed worktree for project \"{}\".",
                project.default_provider.as_str(),
                agent_name,
                project.name
            );
            (
                project.clone(),
                project.default_provider.clone(),
                branch_name.clone(),
                status_message,
                branch_name,
                worktree_path,
                false,
                custom_name,
                true,
            )
        }
        CreateAgentRequest::ForkExternalWorktree {
            project,
            source_worktree_path,
            source_label,
            source_branch,
            custom_name,
        } => {
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Creating a managed worktree from external worktree \"{source_label}\"...",
            )));
            let source_head = match git::head_commit(&source_worktree_path) {
                Ok(head) => head,
                Err(err) => {
                    logger::error(&format!(
                        "failed to resolve HEAD for {}: {err}",
                        source_worktree_path.display()
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to inspect external worktree \"{source_label}\": {err}",
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
                        "external worktree fork creation failed for {}: {err}",
                        project.path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to create a managed worktree from external worktree \"{source_label}\": {err}",
                    )));
                    return;
                }
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Copying dirty and untracked files from external worktree \"{source_label}\"...",
            )));
            if let Err(err) = git::mirror_worktree_contents(&source_worktree_path, &worktree_path) {
                logger::error(&format!(
                    "failed to mirror external worktree {} into {}: {err}",
                    source_worktree_path.display(),
                    worktree_path.display()
                ));
                let _ = git::remove_worktree(&repo_path, &worktree_path, &branch_name);
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Failed to copy external worktree contents from \"{source_label}\": {err}",
                )));
                return;
            }
            let status_message = format!(
                "Created {} agent \"{}\" from external worktree \"{}\" in project \"{}\". Dirty and untracked files were copied into the managed worktree.",
                project.default_provider.as_str(),
                branch_name,
                source_label,
                project.name
            );
            (
                project.clone(),
                project.default_provider.clone(),
                source_branch,
                status_message,
                branch_name,
                worktree_path,
                true,
                None,
                false,
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
    let started_providers = if launch_with_resume {
        vec![provider.as_str().to_string()]
    } else {
        Vec::new()
    };
    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
        provider,
        source_branch,
        branch_name,
        worktree_path: worktree_path.to_string_lossy().to_string(),
        title,
        started_providers,
        desired_running: true,
        auto_reopen_enabled: true,
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
    let launch_message = if launch_with_resume {
        format!(
            "Continuing {} in the existing worktree...",
            session.provider.as_str()
        )
    } else {
        format!(
            "Launching {} in a fresh session...",
            session.provider.as_str()
        )
    };
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(launch_message));
    // crossterm::terminal::size() returns (cols, rows).
    let (cols, rows) = term_size;
    let request = AgentLaunchRequest {
        session,
        provider_config: provider_cfg,
        resume: launch_with_resume,
        pty_size: (rows, cols),
        scrollback_lines: config.ui.agent_scrollback_lines,
        kind: AgentLaunchKind::Create {
            status_message,
            repo_path: repo_path.to_string_lossy().to_string(),
            owns_worktree,
        },
    };
    run_agent_launch_job(request, worker_tx);
}

pub(crate) fn run_agent_launch_job(request: AgentLaunchRequest, worker_tx: Sender<WorkerEvent>) {
    let launch_args = request.provider_config.interactive_args(request.resume);
    let (rows, cols) = request.pty_size;
    logger::debug(&format!(
        "spawning PTY {:?} {:?} in {} ({}x{}, resume_supported={})",
        request.provider_config.command,
        launch_args,
        request.session.worktree_path,
        cols,
        rows,
        request.provider_config.supports_session_resume()
    ));

    let client = match PtyClient::spawn(
        &request.provider_config.command,
        launch_args,
        Path::new(&request.session.worktree_path),
        rows,
        cols,
        request.scrollback_lines,
    ) {
        Ok(client) => client,
        Err(err) => {
            logger::error(&format!(
                "PTY spawn failed for {}: {err}",
                request.session.id
            ));
            if let AgentLaunchKind::Create {
                repo_path,
                owns_worktree,
                ..
            } = &request.kind
                && *owns_worktree
            {
                let _ = git::remove_worktree(
                    Path::new(repo_path),
                    Path::new(&request.session.worktree_path),
                    &request.session.branch_name,
                );
            }
            let message = if matches!(request.kind, AgentLaunchKind::Create { .. }) {
                format!("Failed to start {}: {err}", request.provider_config.command)
            } else {
                err.to_string()
            };
            let _ = worker_tx.send(WorkerEvent::AgentLaunchFailed(Box::new(
                AgentLaunchFailedData { request, message },
            )));
            return;
        }
    };
    logger::info(&format!("PTY session started for {}", request.session.id));
    let _ = worker_tx.send(WorkerEvent::AgentLaunchReady(Box::new(
        AgentLaunchReadyData { request, client },
    )));
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
    let remote = git::remote_github_repo(Path::new(&entry.worktree_path));
    let (host, owner_repo) = if let Some(remote) = remote {
        (remote.host, remote.owner_repo)
    } else if let Some(known) = &entry.known_pr {
        (known.host.clone(), known.owner_repo.clone())
    } else {
        return None;
    };

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
                discover_pr_by_branch(&entry.branch_name, &host, &owner_repo, &entry.session_id)
                && newer.number > known.pr_number
            {
                return Some(newer);
            }
            return reconstruct_from_stored(known);
        }

        // Open PR: refresh its current state via `gh pr view`.
        if let Some(pr) = view_pr_by_number(
            known.pr_number,
            &known.host,
            &known.owner_repo,
            &entry.session_id,
        ) {
            // Also check if a newer PR was opened.
            if let Some(newer) =
                discover_pr_by_branch(&entry.branch_name, &host, &owner_repo, &entry.session_id)
                && newer.number > pr.number
            {
                return Some(newer);
            }
            return Some(pr);
        }
    }

    // No known PR — discover by branch name.
    discover_pr_by_branch(&entry.branch_name, &host, &owner_repo, &entry.session_id)
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
        host: stored.host.clone(),
        owner_repo: stored.owner_repo.clone(),
        url: stored.url.clone(),
    })
}

/// Check a known PR by number using `gh pr view`.
fn view_pr_by_number(
    number: u64,
    host: &str,
    owner_repo: &str,
    session_id: &str,
) -> Option<crate::model::PrInfo> {
    let repo = gh_repo_arg(host, owner_repo);
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &repo,
            "--json",
            "number,state,title,url",
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
    parse_pr_json_object(text.trim(), host, owner_repo)
}

/// Discover a PR by branch name using `gh pr list --state all`.
fn discover_pr_by_branch(
    branch: &str,
    host: &str,
    owner_repo: &str,
    session_id: &str,
) -> Option<crate::model::PrInfo> {
    let repo = gh_repo_arg(host, owner_repo);
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--repo",
            &repo,
            "--state",
            "all",
            "--json",
            "number,state,title,url",
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
    parse_pr_json_value(obj, host, owner_repo)
}

/// Parse a single PR JSON object (from `gh pr view` output).
fn parse_pr_json_object(json: &str, host: &str, owner_repo: &str) -> Option<crate::model::PrInfo> {
    let obj: serde_json::Value = serde_json::from_str(json).ok()?;
    parse_pr_json_value(&obj, host, owner_repo)
}

/// Extract PrInfo from a serde_json::Value.
fn parse_pr_json_value(
    obj: &serde_json::Value,
    host: &str,
    owner_repo: &str,
) -> Option<crate::model::PrInfo> {
    use crate::model::{PrInfo, PrState};

    let number = obj.get("number")?.as_u64()?;
    let state_str = obj.get("state")?.as_str()?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| pull_request_url(host, owner_repo, number));
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
        host: normalize_github_host(host).to_string(),
        owner_repo: owner_repo.to_string(),
        url,
    })
}
fn parse_resolved_pull_request_json(
    json: &str,
    project: Project,
    host: &str,
    owner_repo: &str,
) -> Result<ResolvedPullRequest, String> {
    let obj: serde_json::Value = serde_json::from_str(json.trim())
        .map_err(|err| format!("gh returned invalid PR JSON: {err}"))?;
    let number = obj
        .get("number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "gh PR response did not include a PR number.".to_string())?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let state = obj
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let head_ref_name = obj
        .get("headRefName")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "gh PR response did not include a head branch.".to_string())?
        .to_string();
    Ok(ResolvedPullRequest {
        project,
        host: host.to_string(),
        owner_repo: owner_repo.to_string(),
        number,
        title,
        state,
        head_ref_name,
    })
}

fn pull_request_url(host: &str, owner_repo: &str, number: u64) -> String {
    let host = normalize_github_host(host);
    format!("https://{host}/{owner_repo}/pull/{number}")
}

fn gh_repo_arg(host: &str, owner_repo: &str) -> String {
    let host = normalize_github_host(host);
    if host == "github.com" {
        owner_repo.to_string()
    } else {
        format!("{host}/{owner_repo}")
    }
}

fn normalize_github_host(host: &str) -> &str {
    if host.trim().is_empty() {
        "github.com"
    } else {
        host
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::model::PrState;

    #[test]
    fn fork_worker_requires_name_from_prompt() {
        let tmp = tempdir().expect("tempdir");
        let paths = DuxPaths {
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            root: tmp.path().to_path_buf(),
        };
        let project = Project {
            id: "project-1".to_string(),
            name: "demo".to_string(),
            path: tmp.path().to_string_lossy().to_string(),
            explicit_default_provider: None,
            default_provider: ProviderKind::from_str("codex"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
        };
        let now = Utc::now();
        let source_session = AgentSession {
            id: "session-1".to_string(),
            project_id: project.id.clone(),
            project_path: Some(project.path.clone()),
            provider: ProviderKind::from_str("codex"),
            source_branch: "main".to_string(),
            branch_name: "agent-branch".to_string(),
            worktree_path: tmp.path().join("source").to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
        };
        let (worker_tx, worker_rx) = mpsc::channel();

        run_create_agent_job(
            CreateAgentRequest::ForkSession {
                project,
                source_session: Box::new(source_session),
                source_label: "agent-branch".to_string(),
                custom_name: None,
            },
            paths,
            Config::default(),
            worker_tx,
            (80, 24),
        );

        match worker_rx.recv().expect("worker event") {
            WorkerEvent::CreateAgentFailed(message) => {
                assert_eq!(message, "Forking an agent requires choosing a name first.");
            }
            _ => panic!("expected missing-name failure"),
        }
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn fresh_worker_reports_pre_create_pull_failure_before_worktree_creation() {
        let tmp = tempdir().expect("tempdir");
        let paths = DuxPaths {
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            root: tmp.path().to_path_buf(),
        };
        let project = Project {
            id: "project-1".to_string(),
            name: "demo".to_string(),
            path: tmp.path().join("not-a-repo").to_string_lossy().to_string(),
            explicit_default_provider: None,
            default_provider: ProviderKind::from_str("codex"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
        };
        let (worker_tx, worker_rx) = mpsc::channel();

        run_create_agent_job(
            CreateAgentRequest::NewProject {
                project,
                custom_name: Some("agent-branch".to_string()),
                use_existing_branch: false,
                pull_before_create: true,
            },
            paths,
            Config::default(),
            worker_tx,
            (80, 24),
        );

        match worker_rx.recv().expect("worker event") {
            WorkerEvent::CreateAgentProgress(message) => {
                assert_eq!(
                    message,
                    "Pulling latest changes for project \"demo\" before creating the agent..."
                );
            }
            _ => panic!("expected pre-create pull progress"),
        }
        match worker_rx.recv().expect("worker event") {
            WorkerEvent::CreateAgentFailed(message) => {
                assert!(message.contains(
                    "Failed to pull latest changes for project \"demo\" before creating the agent"
                ));
            }
            _ => panic!("expected pre-create pull failure"),
        }
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn gh_repo_arg_uses_owner_repo_for_github_dot_com() {
        assert_eq!(gh_repo_arg("github.com", "owner/repo"), "owner/repo");
        assert_eq!(gh_repo_arg("", "owner/repo"), "owner/repo");
    }

    #[test]
    fn gh_repo_arg_includes_host_for_enterprise() {
        assert_eq!(
            gh_repo_arg("github.example.com", "owner/repo"),
            "github.example.com/owner/repo"
        );
    }

    #[test]
    fn pull_request_url_defaults_empty_host_to_github_dot_com() {
        assert_eq!(
            pull_request_url("", "owner/repo", 12),
            "https://github.com/owner/repo/pull/12"
        );
    }

    #[test]
    fn parse_pr_json_object_uses_gh_url_when_present() {
        let pr = parse_pr_json_object(
            r#"{"number":42,"state":"OPEN","title":"Demo","url":"https://github.com/owner/repo/pull/42"}"#,
            "github.com",
            "owner/repo",
        )
        .expect("pr");

        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(pr.url, "https://github.com/owner/repo/pull/42");
    }

    #[test]
    fn parse_pr_json_object_falls_back_to_host_url() {
        let pr = parse_pr_json_object(
            r#"{"number":42,"state":"MERGED","title":"Demo"}"#,
            "github.example.com",
            "owner/repo",
        )
        .expect("pr");

        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(pr.url, "https://github.example.com/owner/repo/pull/42");
    }
}
