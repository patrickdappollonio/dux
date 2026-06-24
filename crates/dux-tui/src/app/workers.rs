use std::sync::mpsc::Sender;

use dux_core::engine::{
    AgentLaunchFailedOutcome, AgentLaunchReadyOutcome, AgentLaunchReadyView, AuthUserFinalOutcome,
    BeginDeleteSessionOutcome, BeginDeleteSessionView, DeleteTerminalView, DispatchAgentLaunchView,
    DoDeleteSessionView, EventReaction, FinishDeleteSessionView, ProjectPersistenceOutcome,
    ProjectPersistenceView, ResumeFallbackOutcome, StatusUpdate, WorktreeRemoval,
};

use super::*;

impl App {
    pub(crate) fn drain_events(&mut self) {
        while let Ok(event) = self.engine.worker_rx.try_recv() {
            // A PR-lookup completion carries back the opaque id of the keyed busy
            // its dispatch opened. Capture it (and whether the lookup succeeded)
            // before `process_worker_event` consumes the event, so we can DISMISS
            // that busy once the downstream final is in place: success opens the
            // name prompt (its `set_info` is the visible final), failure produced
            // the engine's error `Status` — in both cases the keyed busy only
            // needs clearing so it never strands to the busy timeout.
            let pr_lookup_completion = match &event {
                WorkerEvent::PullRequestResolved {
                    status_op_id: Some(id),
                    result,
                } => Some((id.clone(), result.is_ok())),
                _ => None,
            };
            let reaction = self.engine.process_worker_event(event);
            self.apply_reaction(reaction);
            if let Some((id, succeeded)) = pr_lookup_completion
                && let Some(op) = self.pending_pr_lookup_ops.remove(&id)
            {
                let outcome = if succeeded {
                    PrLookupFinalOutcome::HandedOff
                } else {
                    PrLookupFinalOutcome::Failed
                };
                self.apply_reaction(op.resolve(&outcome).into_reaction());
            }
        }
        self.retry_hung_resume_sessions();
        // Detect PTY exits.
        let mut exited = Vec::new();
        for (session_id, provider) in &mut self.engine.providers {
            let exit_success = provider.try_wait().map(|status| status.success());
            if exit_success.is_some() || provider.is_exited() {
                let is_minimal = provider.has_minimal_output(5);
                let output = if is_minimal {
                    provider.visible_text_excerpt(usize::MAX)
                } else {
                    String::new()
                };
                exited.push((session_id.clone(), exit_success, is_minimal, output));
            }
        }

        // For sessions that were spawned with resume_args and exited before
        // producing any output, retry with regular args (fresh session).
        // This handles `claude --continue || claude` style fallback.
        // Sessions whose exit was fully handled by a resume-fallback retry (or
        // is protected because a launch is already in flight). These must be
        // skipped by the destructive second loop AND by the post-exit UI/PR
        // follow-ups below.
        let mut handled = HashSet::new();
        for (session_id, _, is_minimal, _) in &exited {
            if !self
                .engine
                .resume_fallback_candidates
                .contains_key(session_id)
            {
                continue;
            }
            if !is_minimal {
                // Non-minimal exit of a resume candidate: preserve today's
                // behavior — drop the candidate unconditionally and let the
                // second loop mark it Detached.
                self.engine.resume_fallback_candidates.remove(session_id);
                continue;
            }
            let Some(session) = self
                .engine
                .sessions
                .iter()
                .find(|s| s.id == *session_id)
                .cloned()
            else {
                // Session gone: drop the stale candidate, fall through.
                self.engine.resume_fallback_candidates.remove(session_id);
                continue;
            };
            let proj_name = self.engine.project_name_for_session(&session);
            let status_message = format!(
                "No prior session to resume for agent \"{}\". Started a fresh {} session in project \"{}\".",
                session.branch_name,
                session.provider.as_str(),
                proj_name,
            );
            logger::info(&format!(
                "resume args exited without output for agent \"{}\", retrying with regular args",
                session.branch_name
            ));
            let pty_size = self.pty_size_for_launch();
            match self
                .engine
                .retry_resume_fallback(session_id, pty_size, status_message)
            {
                ResumeFallbackOutcome::Retried { reaction } => {
                    self.engine.pty_activity.remove(session_id);
                    self.engine.pty_input.remove(session_id);
                    self.apply_reaction(*reaction);
                    handled.insert(session_id.clone());
                }
                ResumeFallbackOutcome::InFlight => {
                    // Protect: a launch is already in flight; do not let the
                    // second loop tear this session down.
                    handled.insert(session_id.clone());
                }
                ResumeFallbackOutcome::NotCandidate => {
                    // Candidate was removed by another path this tick; fall
                    // through to normal exit handling.
                }
            }
        }

        for (session_id, exit_success, _, _) in &exited {
            if handled.contains(session_id) {
                continue;
            }
            self.engine.providers.remove(session_id);
            self.engine.running_provider_pins.remove(session_id);
            self.engine.pty_activity.remove(session_id);
            self.engine.pty_input.remove(session_id);
            if *exit_success == Some(true) {
                self.engine.mark_session_desired_running(session_id, false);
            }
            self.engine
                .mark_session_status(session_id, SessionStatus::Detached);
        }
        if !exited.is_empty() {
            // If the currently-viewed session just exited (and was not handled
            // by a resume-fallback retry), leave interactive mode.
            if let Some(current) = self.selected_session()
                && let Some((_, exit_success, is_minimal, excerpt)) =
                    exited.iter().find(|(id, _, _, _)| id == &current.id)
                && !handled.contains(&current.id)
            {
                let key = self.bindings.label_for(Action::ReconnectAgent);
                if self.session_surface == SessionSurface::Agent {
                    if *is_minimal && !excerpt.trim().is_empty() {
                        let provider = self
                            .engine
                            .running_provider_for(current)
                            .as_str()
                            .to_string();
                        logger::error(&format!(
                            "Agent CLI process for agent \"{}\" ({provider}) exited. Full captured output:\n{}",
                            current.branch_name, excerpt
                        ));
                    }
                    let status =
                        agent_exit_status_message(*exit_success, *is_minimal, excerpt, &key);
                    self.input_target = InputTarget::None;
                    self.fullscreen_overlay = FullscreenOverlay::None;
                    self.focus = FocusPane::Left;
                    if *exit_success == Some(false) {
                        self.set_error(status);
                    } else {
                        self.set_info(status);
                    }
                } else {
                    self.set_info(format!(
                        "Agent CLI process exited. Companion terminal is still available; press \"{key}\" to relaunch the agent."
                    ));
                }
            }
            // Trigger PR status check for exited agents.
            for sid in &exited {
                let session_id = &sid.0;
                if !handled.contains(session_id) {
                    self.engine.spawn_pr_check_for_session(session_id);
                }
            }
        }

        let mut exited_terminal_ids = Vec::new();
        for (terminal_id, terminal) in &mut self.engine.companion_terminals {
            if terminal.client.is_exited() || terminal.client.try_wait().is_some() {
                exited_terminal_ids.push(terminal_id.clone());
            }
        }
        for terminal_id in &exited_terminal_ids {
            self.engine.companion_terminals.remove(terminal_id);
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

        // Refresh companion-terminal foreground commands. The engine throttles
        // this by wall-clock (~2s), so calling it on every ~100ms tick keeps the
        // cadence without coupling the refresh to the tick count.
        self.engine.refresh_terminal_foregrounds();

        // Spawn a background worker to refresh resource monitor stats when
        // the overlay is open and enough wall-clock time has elapsed (~2s).
        if let PromptState::ResourceMonitor {
            ref last_refresh, ..
        } = self.prompt
            && last_refresh.elapsed() >= Duration::from_secs(2)
        {
            self.engine.spawn_resource_stats_worker();
        }

        // Keep the poller's interval flag in sync with whether any runtime PTY is alive.
        self.engine
            .has_active_processes
            .store(self.running_process_count() > 0, Ordering::Relaxed);
    }

    pub(super) fn apply_reaction(&mut self, reaction: EventReaction) {
        match reaction {
            EventReaction::Nothing => {}
            EventReaction::Status(StatusUpdate { tone, message, key }) => {
                // When a `StatusUpdate` carries a key (keyed operation), write it
                // into the named slot so `most_recent_tui` can pick it up.
                // Unkeyed updates (`key == None`) write the anonymous slot.
                // Info entries auto-clear after `clear_after`; Busy persists until
                // replaced; Warning/Error persist until the next status.
                self.status.set(Instant::now(), key, tone, message);
            }
            EventReaction::ClearStatus(key) => {
                // The `Final::Clear` outcome of a StatusOp: dismiss the keyed
                // entry with no replacement message.
                self.status.clear(&key, None);
            }

            EventReaction::Multi(reactions) => {
                for r in reactions {
                    self.apply_reaction(r);
                }
            }
            EventReaction::RebuildLeftItems => self.rebuild_left_items(),
            EventReaction::ReloadChangedFiles => self.reload_changed_files(),
            EventReaction::ClampFilesCursor => self.clamp_files_cursor(),

            EventReaction::AgentLaunchReadyView(boxed) => {
                self.apply_agent_launch_ready_view(*boxed);
            }
            EventReaction::AgentLaunchFailedView(boxed) => {
                self.apply_agent_launch_failed_view(*boxed);
            }

            EventReaction::CommitMessageGenerated {
                session_id: _,
                message,
            } => {
                // Domain only: drop the draft into the input. The user-facing
                // status rides the StatusOp's separate StatusOpCompleted event.
                self.commit_input.clear_overlay();
                self.commit_input.set_text(message);
                self.input_target = InputTarget::CommitMessage;
            }
            EventReaction::CommitMessageFailed {
                session_id: _,
                error: _,
            } => {
                // Domain only: clear the overlay. The failure status rides the
                // StatusOp's separate StatusOpCompleted event.
                self.commit_input.clear_overlay();
            }

            EventReaction::BrowserEntriesArrived { dir, entries } => {
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
            EventReaction::ProjectWorktreesArrived {
                project_id,
                result,
                status_op_id,
            } => {
                // The final depends on whether the picker is still open and
                // matching when the worktrees arrive, a fact the worker can't
                // see; resolve the HandlerStatusOp against that handler-computed
                // outcome. The op (when present) encapsulates each final message
                // declared at dispatch; the keyed `Dismissed` clear removes only
                // this op's busy, so a newer message from another action is never
                // clobbered.
                let mut outcome: Option<WorktreesFinalOutcome> = None;
                if let PromptState::PickProjectWorktree(prompt) = &mut self.prompt
                    && prompt.project.id == project_id
                {
                    prompt.loading = false;
                    match result {
                        Ok(entries) => {
                            prompt.selected = selectable_project_worktree_indices(&entries)
                                .into_iter()
                                .next();
                            prompt.entries = entries;
                            prompt.error = None;
                            outcome = Some(WorktreesFinalOutcome::Loaded);
                        }
                        Err(error) => {
                            prompt.entries.clear();
                            prompt.selected = None;
                            prompt.error = Some(error.clone());
                            outcome = Some(WorktreesFinalOutcome::Failed(error));
                        }
                    }
                }
                // The picker was dismissed or switched before its worktrees
                // loaded, so nothing consumed the result.
                let outcome = outcome.unwrap_or(WorktreesFinalOutcome::Dismissed);
                if let Some(id) = status_op_id
                    && let Some(op) = self.pending_worktree_ops.remove(&id)
                {
                    let resolved = op.resolve(&outcome);
                    self.apply_reaction(resolved.into_reaction());
                }
            }

            EventReaction::OpenNewAgentPromptForPr {
                pr,
                status_op_id: _,
            } => {
                let pr = *pr;
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
            EventReaction::WorktreeRemoveSucceeded {
                session_id,
                branch_already_deleted,
                our_busy_message: _,
            } => {
                // The "Removing worktree …" busy now rides a keyed
                // `HandlerStatusOp` stashed in `pending_delete_ops`, so the keyed
                // final replaces exactly that spinner without comparing it against
                // the anonymous status line — concurrent operations can never
                // clobber it. Pop the op and resolve it against the handler-known
                // outcome; the message wording is unchanged.
                let op = self.pending_delete_ops.remove(&session_id);
                if self.engine.sessions.iter().any(|s| s.id == session_id) {
                    // Cleanup still runs (in-memory + view side); pass
                    // `update_status=false` so it no longer authors the success
                    // line — the op owns the final message now.
                    if let Err(e) = self.finish_delete_session(
                        &session_id,
                        WorktreeRemoval::Performed {
                            branch_already_deleted,
                        },
                        false,
                    ) {
                        self.set_error(format!(
                            "Worktree removed but session cleanup failed: {e:#}"
                        ));
                    } else if let Some(op) = op {
                        self.apply_reaction(
                            op.resolve(&TuiDeleteOutcome::SucceededPresent {
                                branch_already_deleted,
                            })
                            .into_reaction(),
                        );
                    }
                } else if let Some(op) = op {
                    // Session removed by another path. The keyed op can't clobber
                    // unrelated statuses, but preserve the legacy suppression:
                    // emit "Worktree removal finished." only when our busy is still
                    // the anonymous status, otherwise clear with no message.
                    let our_busy_still_showing = self
                        .status
                        .anon_busy_matches(op.pending_status().message.as_str());
                    self.apply_reaction(
                        op.resolve(&TuiDeleteOutcome::SucceededGone {
                            our_busy_still_showing,
                        })
                        .into_reaction(),
                    );
                }
            }
            EventReaction::WorktreeRemoveFailed {
                session_id,
                message,
            } => {
                // Session record is normally still present because we
                // deferred cleanup until git succeeded. The keyed op's resolver
                // captured the session label at dispatch; whether the session is
                // still present at completion selects the named vs bare wording.
                let session_present = self.engine.sessions.iter().any(|s| s.id == session_id);
                if let Some(op) = self.pending_delete_ops.remove(&session_id) {
                    let outcome = if session_present {
                        TuiDeleteOutcome::FailedNamed { message }
                    } else {
                        TuiDeleteOutcome::FailedBare { message }
                    };
                    self.apply_reaction(op.resolve(&outcome).into_reaction());
                }
            }

            EventReaction::ResourceStatsArrived(stats) => {
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

            EventReaction::AddProjectAfterBranchCheckout {
                path,
                name,
                target_branch,
                leading_branch,
                status_op_id: _,
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

            EventReaction::ContinueCreateAgentAfterInspection {
                project,
                inspection,
            } => {
                let project_name = project.name.clone();
                match self.sync_projects_to_store_and_update_config() {
                    Ok(()) => {
                        if let Err(err) = self
                            .engine
                            .config_writer
                            .save_eager(self.engine.config.clone())
                        {
                            self.set_error(format!(
                                "Project branch was detected, but config.toml could not be updated: {err}"
                            ));
                        }
                    }
                    Err(err) => {
                        self.set_error(format!(
                            "Project branch was detected, but config.toml could not be updated: {err:#}"
                        ));
                    }
                }
                if let Err(err) =
                    self.continue_create_agent_after_branch_inspection(project, inspection)
                {
                    self.set_error(format!("{err:#}"));
                } else {
                    self.set_info(format!(
                        "Branch check complete for \"{project_name}\". Confirm or edit the agent name to continue."
                    ));
                }
            }
            EventReaction::DispatchProjectDefaultBranchCheckout {
                project,
                default_branch,
                status_op_id: _,
            } => {
                self.dispatch_non_default_branch_checkout(
                    NonDefaultBranchAction::CheckoutProjectDefault { project },
                    default_branch,
                    "for the selected project".to_string(),
                );
            }

            EventReaction::ApplyReloadedConfig(boxed) => {
                if let Err(err) = self.apply_reloaded_config(*boxed) {
                    self.set_error(format!(
                        "Config validation passed, but applying it failed: {err:#}"
                    ));
                } else {
                    self.set_info("Configuration reloaded. New settings are active now.");
                }
            }
            EventReaction::OpenConfigReloadFailedModal(message) => {
                self.open_config_reload_failed_modal(message);
                self.set_error("Config reload failed. Review the modal before retrying.");
            }

            EventReaction::ProjectPersistenceOutcome(boxed) => {
                self.apply_project_persistence_outcome(*boxed);
            }

            EventReaction::AuthUsersOutcome {
                outcome,
                status_op_id,
            } => {
                self.apply_auth_users_outcome(outcome, status_op_id);
            }

            EventReaction::StartupLogArrived { scope_label, log } => {
                self.input_target = InputTarget::None;
                self.terminal_selection = None;
                self.prompt = PromptState::None;
                self.fullscreen_overlay = FullscreenOverlay::StartupLog;
                self.startup_log_viewer = Some(StartupLogViewer {
                    scope_label,
                    path: log.path,
                    display_name: log.display_name,
                    content: log.content,
                    scroll_offset: 0,
                    search: TextInput::new(),
                    searching: false,
                });
                // Domain only: the overlay is now up. The "Opened startup command
                // logs…" confirmation (resolving the busy) rides the StatusOp's
                // separate StatusOpCompleted event.
            }

            EventReaction::FinishDeleteSessionView(view) => {
                let FinishDeleteSessionView {
                    session_id,
                    outcome,
                    removal,
                    update_status,
                } = *view;
                self.apply_finish_delete_session_outcome(
                    &session_id,
                    outcome,
                    removal,
                    update_status,
                );
            }

            EventReaction::DoDeleteSessionView(view) => {
                let DoDeleteSessionView {
                    session_id,
                    outcome,
                } = *view;
                self.apply_finish_delete_session_outcome(
                    &session_id,
                    outcome.finish,
                    outcome.removal,
                    true,
                );
            }

            EventReaction::BeginDeleteSessionView(view) => {
                let BeginDeleteSessionView {
                    session_id,
                    outcome,
                } = *view;
                match outcome {
                    BeginDeleteSessionOutcome::AlreadyInFlight => {
                        self.set_error(
                            "Deletion already in progress for this agent. Wait for it to finish.",
                        );
                    }
                    BeginDeleteSessionOutcome::NotFound => {}
                    BeginDeleteSessionOutcome::AsyncStarted { busy_message } => {
                        // Mint a keyed HandlerStatusOp, show its pending busy, and
                        // stash it keyed by session id so the completion handler
                        // resolves exactly this spinner.
                        let op = self.build_delete_status_op(&session_id, busy_message);
                        self.apply_reaction(EventReaction::Status(op.pending_status()));
                        self.pending_delete_ops.insert(session_id.clone(), op);
                    }
                    BeginDeleteSessionOutcome::Inline { removal } => {
                        if let Err(e) = self.finish_delete_session(&session_id, removal, true) {
                            self.set_error(format!("{e:#}"));
                        }
                    }
                }
            }

            EventReaction::DispatchAgentLaunchView(view) => {
                let DispatchAgentLaunchView {
                    session_id: _,
                    launched: _,
                    status,
                } = *view;
                if let Some(status) = status {
                    self.apply_reaction(EventReaction::Status(status));
                }
                // The `launched` bool is consumed by the App wrapper before
                // `apply_reaction` is called; `session_id` is currently only
                // useful to downstream observers (web layer).
            }

            EventReaction::DeleteTerminalView(view) => {
                let DeleteTerminalView { terminal_id, label } = *view;
                if self.active_terminal_id.as_deref() == Some(terminal_id.as_str()) {
                    self.active_terminal_id = None;
                }
                self.clamp_terminal_cursor();
                if let Some(label) = label {
                    self.set_info(format!("Deleted terminal \"{label}\""));
                }
            }

            EventReaction::ServerFlipPreflightReady { result, warning } => {
                // The worker has reported back: clear the in-flight guard on BOTH
                // arms so a later (legitimate) retry can spawn a fresh pre-flight.
                self.server_flip_preflight_pending = false;
                match result {
                    Ok((listeners, urls)) => {
                        // Surface the warning (if any) first, then announce the
                        // serve URLs; the flip happens on the next loop iteration.
                        let url_list = urls.join(", ");
                        match warning {
                            Some(warn) => self.set_warning(format!(
                                "{warn} Starting the web server on {url_list} — your agents keep running."
                            )),
                            None => self.set_busy(format!(
                                "Starting the web server on {url_list} — your agents keep running."
                            )),
                        }
                        self.pending_server_flip = Some((listeners, urls));
                    }
                    Err(err) => {
                        self.set_error(err);
                    }
                }
            }
        }
    }

    /// Resolve a project-persistence [`HandlerStatusOp`] (stashed at dispatch by
    /// its opaque id) against the handler-computed [`PersistFinalOutcome`] and
    /// apply the resulting keyed final. Returns `true` when an op was found and
    /// resolved; `false` when there was no id or no matching op (the Add inline
    /// path and the web path don't drive a handler-resolved op), so the caller
    /// can fall back to its legacy `set_info`/`set_error`.
    fn resolve_persist_op(
        &mut self,
        status_op_id: &Option<String>,
        outcome: PersistFinalOutcome,
    ) -> bool {
        let Some(id) = status_op_id else {
            return false;
        };
        let Some(op) = self.pending_persist_ops.remove(id) else {
            return false;
        };
        let resolved = op.resolve(&outcome);
        self.apply_reaction(resolved.into_reaction());
        true
    }

    /// Resolve a web-UI login-user add against its handler-computed
    /// [`AuthUserFinalOutcome`]. When the correlation id matches a stashed
    /// [`HandlerStatusOp`] (the normal TUI add path), pop and resolve it into the
    /// keyed final. Otherwise (no id, or an op the surface never minted) fall
    /// back to building the `StatusUpdate` directly via
    /// [`AuthUserFinalOutcome::into_status`] — the same tones and byte-identical
    /// messages the resolver would produce.
    pub(crate) fn apply_auth_users_outcome(
        &mut self,
        outcome: AuthUserFinalOutcome,
        status_op_id: Option<String>,
    ) {
        if let Some(id) = &status_op_id
            && let Some(op) = self.pending_auth_ops.remove(id)
        {
            let resolved = op.resolve(&outcome);
            self.apply_reaction(resolved.into_reaction());
            return;
        }
        self.apply_reaction(EventReaction::Status(outcome.into_status(status_op_id)));
    }

    pub(crate) fn apply_project_persistence_outcome(&mut self, outcome: ProjectPersistenceOutcome) {
        let ProjectPersistenceOutcome {
            action,
            view,
            status_op_id,
        } = outcome;

        match view {
            ProjectPersistenceView::PersistenceFailed { error } => {
                // The op (when present) encapsulates the per-action db-failure
                // message; resolve it so the keyed busy is replaced. Fall back to
                // the legacy direct set for the Add inline / web paths.
                if self
                    .resolve_persist_op(&status_op_id, PersistFinalOutcome::DbFailed(error.clone()))
                {
                    return;
                }
                let msg = match action {
                    ProjectPersistenceAction::Add { project, .. } => format!(
                        "Could not save project \"{}\" to the database: {error}",
                        project.name,
                    ),
                    ProjectPersistenceAction::Remove { project_name, .. } => format!(
                        "Could not remove project \"{project_name}\" from the database: {error}"
                    ),
                    ProjectPersistenceAction::Delete { project_name, .. } => format!(
                        "Could not finish deleting project \"{project_name}\" from the database: {error}"
                    ),
                    ProjectPersistenceAction::UpdateDefaultProvider { project_name, .. } => {
                        format!(
                            "Could not save the provider change for project \"{project_name}\": {error}"
                        )
                    }
                    ProjectPersistenceAction::UpdateAutoReopen { project_name, .. } => format!(
                        "Could not save the auto-reopen change for project \"{project_name}\": {error}"
                    ),
                    ProjectPersistenceAction::UpdateStartupCommand { project_name, .. } => format!(
                        "Could not save the startup command for project \"{project_name}\": {error}"
                    ),
                    ProjectPersistenceAction::UpdateEnv { project_name, .. } => format!(
                        "Could not save environment variables for project \"{project_name}\": {error}"
                    ),
                };
                self.set_error(msg);
            }

            ProjectPersistenceView::Added {
                project_id,
                status_message,
            } => {
                self.rebuild_left_items();
                if let Some(index) = self.left_items().iter().position(|item| {
                    matches!(item, LeftItem::Project(project_index) if self.engine.projects[*project_index].id == project_id)
                }) {
                    self.selected_left = index;
                }
                // The freshly added project is now selected and has no agents,
                // so refresh the right-pane file lists. Without this, the
                // previously selected project's changed files linger and look
                // like they belong to the brand-new project.
                self.reload_changed_files();
                // Add is INLINE: the engine handler already wrote config.toml
                // through the eager queue (with SQLite rollback on failure). Do
                // NOT write it a second time here — that would be a double write.
                // The other arms route their config write through save_eager via
                // update_config_projects_from_runtime (Task 7).
                self.set_info(status_message);
            }

            ProjectPersistenceView::Removed { project_name } => {
                self.rebuild_left_items();
                self.selected_left = self.selected_left.saturating_sub(1);
                // Selection moved to a different item; refresh the right-pane
                // file lists so they match the new selection instead of the
                // removed project's stale changes.
                self.reload_changed_files();
                self.update_config_projects_from_runtime();
                if let Err(err) = self
                    .engine
                    .config_writer
                    .save_eager(self.engine.config.clone())
                {
                    let err = err.to_string();
                    if self.resolve_persist_op(
                        &status_op_id,
                        PersistFinalOutcome::ConfigWriteFailed(err.clone()),
                    ) {
                        return;
                    }
                    self.set_error(format!(
                        "Project was removed from the database, but config.toml could not be updated: {err}"
                    ));
                    return;
                }
                if self.resolve_persist_op(&status_op_id, PersistFinalOutcome::Saved) {
                    return;
                }
                self.set_info(format!("Removed project \"{project_name}\" from app"));
            }

            ProjectPersistenceView::Deleted { project_name } => {
                self.rebuild_left_items();
                self.selected_left = self.selected_left.saturating_sub(1);
                self.reload_changed_files();
                self.update_config_projects_from_runtime();
                if let Err(err) = self
                    .engine
                    .config_writer
                    .save_eager(self.engine.config.clone())
                {
                    let err = err.to_string();
                    if self.resolve_persist_op(
                        &status_op_id,
                        PersistFinalOutcome::ConfigWriteFailed(err.clone()),
                    ) {
                        return;
                    }
                    self.set_error(format!(
                        "Project was deleted from the database, but config.toml could not be updated: {err}"
                    ));
                    return;
                }
                if self.resolve_persist_op(&status_op_id, PersistFinalOutcome::Saved) {
                    return;
                }
                self.set_info(format!(
                    "Deleted project \"{project_name}\" and all its agents"
                ));
            }

            ProjectPersistenceView::DefaultProviderUpdated {
                project_name,
                provider,
                global_default,
            } => {
                self.rebuild_left_items();
                self.update_config_projects_from_runtime();
                if let Err(err) = self
                    .engine
                    .config_writer
                    .save_eager(self.engine.config.clone())
                {
                    let err = err.to_string();
                    if self.resolve_persist_op(
                        &status_op_id,
                        PersistFinalOutcome::ConfigWriteFailed(err.clone()),
                    ) {
                        return;
                    }
                    self.set_error(format!(
                        "Provider preference saved to the database for \"{project_name}\", but config.toml could not be updated: {err}"
                    ));
                    return;
                }
                if self.resolve_persist_op(&status_op_id, PersistFinalOutcome::Saved) {
                    return;
                }
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

            ProjectPersistenceView::AutoReopenUpdated {
                project_name,
                auto_reopen_agents,
            } => {
                self.update_config_projects_from_runtime();
                if let Err(err) = self
                    .engine
                    .config_writer
                    .save_eager(self.engine.config.clone())
                {
                    let err = err.to_string();
                    if self.resolve_persist_op(
                        &status_op_id,
                        PersistFinalOutcome::ConfigWriteFailed(err.clone()),
                    ) {
                        return;
                    }
                    self.set_error(format!(
                        "Auto-reopen preference saved to the database for \"{project_name}\", but config.toml could not be updated: {err}"
                    ));
                    return;
                }
                if self.resolve_persist_op(&status_op_id, PersistFinalOutcome::Saved) {
                    return;
                }
                let enabled = auto_reopen_agents.unwrap_or(true);
                self.set_info(format!(
                    "Startup auto-reopen {} for project \"{}\".",
                    if enabled { "enabled" } else { "disabled" },
                    project_name,
                ));
            }

            ProjectPersistenceView::StartupCommandUpdated {
                project_name,
                startup_command,
            } => {
                self.update_config_projects_from_runtime();
                if let Err(err) = self
                    .engine
                    .config_writer
                    .save_eager(self.engine.config.clone())
                {
                    let err = err.to_string();
                    if self.resolve_persist_op(
                        &status_op_id,
                        PersistFinalOutcome::ConfigWriteFailed(err.clone()),
                    ) {
                        return;
                    }
                    self.set_error(format!(
                        "Startup command saved to the database for \"{project_name}\", but config.toml could not be updated: {err}"
                    ));
                    return;
                }
                if self.resolve_persist_op(&status_op_id, PersistFinalOutcome::Saved) {
                    return;
                }
                match startup_command {
                    Some(command) => self.set_info(format!(
                        "Startup command for project \"{project_name}\" set to: {command}"
                    )),
                    None => self.set_info(format!(
                        "Startup command cleared for project \"{project_name}\"."
                    )),
                }
            }

            ProjectPersistenceView::EnvUpdated {
                project_name,
                env_count,
            } => {
                self.update_config_projects_from_runtime();
                if let Err(err) = self
                    .engine
                    .config_writer
                    .save_eager(self.engine.config.clone())
                {
                    let err = err.to_string();
                    if self.resolve_persist_op(
                        &status_op_id,
                        PersistFinalOutcome::ConfigWriteFailed(err.clone()),
                    ) {
                        return;
                    }
                    self.set_error(format!(
                        "Environment variables saved to the database for \"{project_name}\", but config.toml could not be updated: {err}"
                    ));
                    return;
                }
                if self.resolve_persist_op(&status_op_id, PersistFinalOutcome::Saved) {
                    return;
                }
                if env_count == 0 {
                    self.set_info(format!(
                        "Environment variables cleared for project \"{project_name}\"."
                    ));
                } else {
                    self.set_info(format!(
                        "Saved {env_count} environment variable(s) for project \"{project_name}\". New agents and terminals will receive them.",
                    ));
                }
            }
        }
    }

    fn apply_agent_launch_ready_view(&mut self, outcome: AgentLaunchReadyOutcome) {
        self.last_pty_size = outcome.pty_size;
        if let Some(id) = outcome.detached_session_id {
            self.engine.pty_activity.remove(&id);
            self.engine.pty_input.remove(&id);
        }
        match outcome.view {
            AgentLaunchReadyView::CreatePersistFailed { error } => {
                // Keyed final so it replaces the engine's `create:{id}` busy
                // rather than leaving the spinner to time out.
                let key = dux_core::wire::status_keys::create(&outcome.session.project_id);
                self.set_error_keyed(key, format!("Failed to persist session: {error}"));
            }
            AgentLaunchReadyView::CreateCommitted {
                status_message,
                startup_result_error,
            } => {
                self.rebuild_left_items();
                self.selected_left = self
                    .left_items()
                    .iter()
                    .position(|item| matches!(item, LeftItem::Session(index) if self.engine.sessions.get(*index).map(|candidate| candidate.id.as_str()) == Some(outcome.session.id.as_str())))
                    .unwrap_or(0);
                self.reload_changed_files();
                self.show_agent_surface();
                self.input_target = InputTarget::Agent;
                self.fullscreen_overlay = FullscreenOverlay::Agent;
                // Keyed finals so the create success/startup-error replaces the
                // engine's `create:{id}` busy entry instead of stranding it.
                let key = dux_core::wire::status_keys::create(&outcome.session.project_id);
                if let Some(err) = startup_result_error {
                    self.set_error_keyed(key, format!(
                        "Startup command failed for agent \"{}\": {err}. Run read-startup-command-logs for details.",
                        outcome.session.branch_name
                    ));
                } else {
                    self.set_info_keyed(key, status_message);
                }
            }
            AgentLaunchReadyView::SessionMissing => {
                // The session vanished between dispatch and launch. Resolve any
                // open launch/create busy so its spinner doesn't linger: drop the
                // keyed create entry, and clear a still-showing anon launch busy.
                self.status.clear(
                    &dux_core::wire::status_keys::create(&outcome.session.project_id),
                    None,
                );
                if matches!(self.status.most_recent_tui(), Some((StatusTone::Busy, _))) {
                    self.set_info(String::new());
                }
            }
            AgentLaunchReadyView::Reconnect { status_message } => {
                self.show_agent_surface();
                self.input_target = InputTarget::Agent;
                self.fullscreen_overlay = FullscreenOverlay::Agent;
                self.set_info(status_message);
            }
            AgentLaunchReadyView::ResumeFallback {
                session_id,
                status_message,
            } => {
                if self.selected_session().map(|selected| selected.id.as_str())
                    == Some(session_id.as_str())
                {
                    self.show_agent_surface();
                    self.input_target = InputTarget::Agent;
                    self.fullscreen_overlay = FullscreenOverlay::Agent;
                }
                self.set_info(status_message);
            }
            AgentLaunchReadyView::StartupAutoReopen => {}
        }
    }

    fn apply_agent_launch_failed_view(&mut self, outcome: AgentLaunchFailedOutcome) {
        match outcome {
            AgentLaunchFailedOutcome::Create {
                project_id,
                message,
            } => {
                // Keyed to the create op so a launch failure after the worktree
                // was created still replaces the `create:{id}` busy spinner.
                self.set_error_keyed(dux_core::wire::status_keys::create(&project_id), message)
            }
            AgentLaunchFailedOutcome::Reconnect {
                branch_name,
                message,
                ..
            } => {
                self.set_error(format!(
                    "Reconnect failed for agent \"{branch_name}\": {message}"
                ));
            }
            AgentLaunchFailedOutcome::ForceReconnect {
                branch_name,
                message,
                ..
            } => {
                self.set_error(format!(
                    "Fresh restart failed for agent \"{branch_name}\": {message}"
                ));
            }
            AgentLaunchFailedOutcome::ResumeFallback => {
                // Engine logged + marked Detached; nothing for the view.
            }
            AgentLaunchFailedOutcome::StartupAutoReopen {
                branch_name,
                message,
                ..
            } => {
                self.set_warning(format!(
                    "Couldn't auto-reopen agent \"{branch_name}\": {message}"
                ));
            }
        }
    }

    fn retry_hung_resume_sessions(&mut self) {
        let mut hung = Vec::new();

        for (session_id, started_at) in &self.engine.resume_fallback_candidates {
            let Some(session) = self.engine.sessions.iter().find(|s| s.id == *session_id) else {
                continue;
            };
            let cfg = provider_config(&self.engine.config, &session.provider);
            let Some(timeout_ms) = cfg.resume_wait_timeout_ms.filter(|timeout| *timeout > 0) else {
                continue;
            };
            if started_at.elapsed() < Duration::from_millis(timeout_ms) {
                continue;
            }
            let Some(provider) = self.engine.providers.get(session_id) else {
                continue;
            };
            if provider.has_output() {
                continue;
            }
            hung.push(session_id.clone());
        }

        for session_id in hung {
            let Some(session) = self
                .engine
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .cloned()
            else {
                // Session vanished between detection and retry; drop any stale
                // candidate so it can't leak.
                self.engine.resume_fallback_candidates.remove(&session_id);
                continue;
            };
            let proj_name = self.engine.project_name_for_session(&session);
            let status_message = format!(
                "Resume timed out for agent \"{}\" with no visible output. Started a fresh {} session in project \"{}\".",
                session.branch_name,
                session.provider.as_str(),
                proj_name,
            );
            logger::info(&format!(
                "resume args produced no visible output for agent \"{}\" within timeout, retrying with regular args",
                session.branch_name
            ));
            let pty_size = self.pty_size_for_launch();
            match self
                .engine
                .retry_resume_fallback(&session_id, pty_size, status_message)
            {
                ResumeFallbackOutcome::Retried { reaction } => {
                    self.engine.pty_activity.remove(&session_id);
                    self.engine.pty_input.remove(&session_id);
                    self.apply_reaction(*reaction);
                }
                // InFlight: a launch is already in progress — leave it alone.
                // NotCandidate: nothing to retry (engine cleared any stale
                // candidate). Either way, no further action here.
                ResumeFallbackOutcome::InFlight | ResumeFallbackOutcome::NotCandidate => {}
            }
        }
    }
}

fn agent_exit_status_message(
    exit_success: Option<bool>,
    is_minimal: bool,
    excerpt: &str,
    reconnect_key: &str,
) -> String {
    const MAX_EXIT_OUTPUT_CHARS: usize = 120;

    let outcome = match exit_success {
        Some(false) => "exited with an error",
        Some(true) => "exited",
        None => "exited",
    };
    let output = excerpt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if output.is_empty() {
        return format!("Agent CLI process has exited. Press \"{reconnect_key}\" to relaunch.");
    }
    if is_minimal {
        let output = truncate_status_output(&output, MAX_EXIT_OUTPUT_CHARS);
        let more = if output.truncated {
            " Full output was written to the logs."
        } else {
            ""
        };
        return format!(
            "Agent CLI process {outcome}. Output: {}.{more} Press \"{reconnect_key}\" to relaunch.",
            output.text
        );
    }

    format!("Agent CLI process has exited. Press \"{reconnect_key}\" to relaunch.")
}

struct TruncatedStatusOutput {
    text: String,
    truncated: bool,
}

fn truncate_status_output(text: &str, max_chars: usize) -> TruncatedStatusOutput {
    let mut chars = text.chars();
    let mut truncated = false;
    let mut output = String::new();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return TruncatedStatusOutput {
                text: output,
                truncated,
            };
        };
        output.push(ch);
    }
    if chars.next().is_some() {
        truncated = true;
        output.push('…');
    }
    TruncatedStatusOutput {
        text: output,
        truncated,
    }
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;

    fn test_session(worktree: &Path) -> AgentSession {
        AgentSession {
            id: "session-1".to_string(),
            project_id: "project-1".to_string(),
            project_path: Some(worktree.to_string_lossy().to_string()),
            provider: ProviderKind::from_str("custom"),
            source_branch: "main".to_string(),
            branch_name: "agent-branch".to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// `EventReaction::ClearStatus` (the `Final::Clear` outcome of a StatusOp)
    /// must remove the keyed entry with no replacement.
    #[test]
    fn clear_status_reaction_dismisses_the_keyed_entry() {
        use crate::statusline::StatusTone;
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        app.status.set(
            std::time::Instant::now(),
            Some("push:/a".to_string()),
            StatusTone::Busy,
            "Pushing\u{2026}",
        );
        app.apply_reaction(dux_core::engine::EventReaction::ClearStatus(
            "push:/a".into(),
        ));
        assert!(
            app.status
                .snapshot()
                .iter()
                .all(|s| s.key.as_deref() != Some("push:/a")),
            "ClearStatus must remove the keyed entry"
        );
    }

    /// A successful create launch must replace the engine's keyed `create:{id}`
    /// Busy with a same-key Info final, not an anonymous one — otherwise the
    /// keyed Busy entry lingers and times out to a spurious Warning.
    #[test]
    fn create_committed_replaces_the_keyed_create_busy() {
        use crate::statusline::StatusTone;

        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();
        let project_id = session.project_id.clone();
        let create_key = dux_core::wire::status_keys::create(&project_id);

        // Simulate the engine's keyed create Busy.
        app.status.set(
            std::time::Instant::now(),
            Some(create_key.clone()),
            StatusTone::Busy,
            "Creating worktree…",
        );
        assert_eq!(
            app.status
                .snapshot()
                .iter()
                .find(|s| s.key.as_deref() == Some(create_key.as_str()))
                .map(|s| s.tone.as_str()),
            Some("busy"),
        );

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            session,
            pty_size: (80, 24),
            detached_session_id: None,
            view: AgentLaunchReadyView::CreateCommitted {
                status_message: "Created agent.".to_string(),
                startup_result_error: None,
            },
        });

        let entry_tone = app
            .status
            .snapshot()
            .into_iter()
            .find(|s| s.key.as_deref() == Some(create_key.as_str()))
            .map(|s| s.tone);
        assert_eq!(
            entry_tone.as_deref(),
            Some("info"),
            "the create Busy must be replaced in place by a same-key Info final",
        );
    }

    #[test]
    fn launch_job_fails_before_pty_when_provider_command_is_missing() {
        let tmp = tempdir().expect("tempdir");
        let (worker_tx, worker_rx) = mpsc::channel();
        let request = AgentLaunchRequest {
            session: test_session(tmp.path()),
            provider_config: crate::config::ProviderCommandConfig {
                command: "definitely-missing-provider-command".to_string(),
                args: vec!["--ignored".to_string()],
                ..Default::default()
            },
            resume: false,
            pty_size: (24, 80),
            scrollback_lines: 1_000,
            env: Vec::new(),
            kind: AgentLaunchKind::Reconnect {
                status_message: "reconnect".to_string(),
            },
        };

        dux_core::agent_job::run_agent_launch_job(request, worker_tx);

        match worker_rx.recv().expect("worker event") {
            WorkerEvent::AgentLaunchFailed(data) => {
                assert!(data.message.contains("definitely-missing-provider-command"));
                assert!(data.message.contains("not found on PATH"));
            }
            _ => panic!("expected launch failure"),
        }
        assert!(worker_rx.try_recv().is_err());
    }

    #[test]
    fn agent_exit_status_message_caps_long_provider_output() {
        let long_output = "x".repeat(200);

        let message = agent_exit_status_message(Some(false), true, &long_output, "r");

        assert!(message.contains("Output: "));
        assert!(message.contains("…"));
        assert!(message.contains("Full output was written to the logs."));
        assert!(
            !message.contains(&long_output),
            "status should not embed the full provider output"
        );
    }

    #[test]
    fn agent_exit_status_message_concats_short_provider_output() {
        let message = agent_exit_status_message(Some(false), true, "first\nsecond", "r");

        assert!(message.contains("Output: first second."));
        assert!(!message.contains('|'));
        assert!(!message.contains("Full output was written"));
    }

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
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
            created_at: None,
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

        dux_core::agent_job::run_create_agent_job(
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
            WorkerEvent::CreateAgentFailed { message, .. } => {
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
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
            created_at: None,
        };
        let (worker_tx, worker_rx) = mpsc::channel();

        dux_core::agent_job::run_create_agent_job(
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
            WorkerEvent::CreateAgentProgress { key, message } => {
                assert_eq!(key, "create:project-1");
                assert_eq!(
                    message,
                    "Pulling latest changes for project \"demo\" before creating the agent..."
                );
            }
            _ => panic!("expected pre-create pull progress"),
        }
        match worker_rx.recv().expect("worker event") {
            WorkerEvent::CreateAgentFailed { message, .. } => {
                assert!(message.contains(
                    "Failed to pull latest changes for project \"demo\" before creating the agent"
                ));
            }
            _ => panic!("expected pre-create pull failure"),
        }
        assert!(worker_rx.try_recv().is_err());
    }
}
