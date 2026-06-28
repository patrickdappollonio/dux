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
            // The three checkout / branch-inspection completions carry back the
            // opaque id of the keyed busy their dispatch opened (see
            // `pending_checkout_inspect_ops`). Capture it before the event is
            // consumed so we can DISMISS that busy once the visible final is in
            // place. The op resolves to a clear in every terminal case — EXCEPT the
            // checkout-default inspection's Known case, which chains into worker 2
            // (`DispatchProjectDefaultBranchCheckout`): that handler keeps the same
            // op alive across the chain, so we skip resolution here when the
            // reaction is the chain handoff.
            let checkout_inspect_completion = match &event {
                WorkerEvent::NonDefaultBranchCheckoutCompleted {
                    status_op_id: Some(id),
                    ..
                }
                | WorkerEvent::CreateAgentBranchInspected {
                    status_op_id: Some(id),
                    ..
                }
                | WorkerEvent::CheckoutProjectDefaultBranchInspected {
                    status_op_id: Some(id),
                    ..
                } => Some(id.clone()),
                _ => None,
            };
            let reaction = self.engine.process_worker_event(event);
            let chains_forward = matches!(
                reaction,
                EventReaction::DispatchProjectDefaultBranchCheckout { .. }
            );
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
            if let Some(id) = checkout_inspect_completion
                && !chains_forward
                && let Some(op) = self.pending_checkout_inspect_ops.remove(&id)
            {
                self.apply_reaction(op.resolve(&TuiCheckoutInspectOutcome::Done).into_reaction());
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
            EventReaction::Status(StatusUpdate {
                tone, message, key, ..
            }) => {
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
                status_op_id,
            } => {
                // The checkout-default chain: ONE op spans worker 1 (inspection)
                // and worker 2 (the switch). Re-emit the carried op's busy with
                // worker 2's text via `progress` (same opaque id, so the spinner is
                // continuous), then forward the id so worker 2's completion resolves
                // exactly this op. If no op rode along (e.g. a future caller passes
                // `None`), fall back to minting a fresh op inside the dispatch.
                let path = NonDefaultBranchAction::CheckoutProjectDefault {
                    project: project.clone(),
                }
                .repo_path()
                .to_string();
                if let Some(id) = &status_op_id
                    && let Some(op) = self.pending_checkout_inspect_ops.get(id)
                {
                    let progress = op.progress(format!(
                        "Checking out \"{default_branch}\" in {path} for the selected project..."
                    ));
                    self.apply_reaction(EventReaction::Status(progress));
                }
                self.dispatch_non_default_branch_checkout(
                    NonDefaultBranchAction::CheckoutProjectDefault { project },
                    default_branch,
                    "for the selected project".to_string(),
                    status_op_id,
                );
            }

            EventReaction::ApplyReloadedConfig(boxed) => {
                // Resolve the TUI's keyed reload busy op (if one rode along) into
                // its keyed final, REPLACING the legacy `set_info`/`set_error` with
                // byte-identical messages. The shared engine reload logic is
                // untouched. Fall back to the legacy calls if no op was stashed.
                let outcome = match self.apply_reloaded_config(*boxed) {
                    Err(err) => TuiConfigReloadOutcome::ApplyFailed(format!("{err:#}")),
                    Ok(()) => TuiConfigReloadOutcome::Applied,
                };
                if let Some(op) = self.pending_config_reload_op.take() {
                    self.apply_reaction(op.resolve(&outcome).into_reaction());
                } else {
                    match outcome {
                        TuiConfigReloadOutcome::Applied => {
                            self.set_info("Configuration reloaded. New settings are active now.");
                        }
                        TuiConfigReloadOutcome::ApplyFailed(err) => {
                            self.set_error(format!(
                                "Config validation passed, but applying it failed: {err}"
                            ));
                        }
                        TuiConfigReloadOutcome::ValidationFailed => {}
                    }
                }
            }
            EventReaction::OpenConfigReloadFailedModal(message) => {
                self.open_config_reload_failed_modal(message);
                if let Some(op) = self.pending_config_reload_op.take() {
                    self.apply_reaction(
                        op.resolve(&TuiConfigReloadOutcome::ValidationFailed)
                            .into_reaction(),
                    );
                } else {
                    self.set_error("Config reload failed. Review the modal before retrying.");
                }
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
                // The flip's keyed busy op was stashed at dispatch; resolve/advance
                // it here so its spinner is never stranded. Plain success re-emits
                // the busy with the serve URLs via `progress` (same id) and LEAVES
                // the op stashed — the spinner shows until the run loop flips; the
                // warning/error arms resolve the op into a keyed final (byte-
                // identical to the legacy `set_warning`/`set_error`).
                match result {
                    Ok((listeners, urls)) => {
                        // Surface the warning (if any) first, then announce the
                        // serve URLs; the flip happens on the next loop iteration.
                        let url_list = urls.join(", ");
                        match warning {
                            Some(warn) => {
                                if let Some(op) = self.pending_server_flip_op.take() {
                                    self.apply_reaction(
                                        op.resolve(&TuiServerFlipOutcome::Warned(format!(
                                            "{warn} Starting the web server on {url_list} — your agents keep running."
                                        )))
                                        .into_reaction(),
                                    );
                                }
                            }
                            None => {
                                if let Some(op) = &self.pending_server_flip_op {
                                    let progress = op.progress(format!(
                                        "Starting the web server on {url_list} — your agents keep running."
                                    ));
                                    self.apply_reaction(EventReaction::Status(progress));
                                }
                            }
                        }
                        self.pending_server_flip = Some((listeners, urls));
                    }
                    Err(err) => {
                        if let Some(op) = self.pending_server_flip_op.take() {
                            self.apply_reaction(
                                op.resolve(&TuiServerFlipOutcome::Failed(err))
                                    .into_reaction(),
                            );
                        }
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
            AgentLaunchReadyView::CreatePersistFailed { .. } => {
                // The create op's keyed error final is resolved ENGINE-SIDE and
                // arrives alongside this View as a sibling `Status` in the same
                // `Multi`, so there is no status to set here.
            }
            AgentLaunchReadyView::CreateCommitted {
                status_message: _,
                startup_result_error: _,
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
                // The create success / startup-error keyed final is resolved
                // ENGINE-SIDE and arrives as a sibling `Status` in the same
                // `Multi`; this arm keeps only the non-status view work.
            }
            AgentLaunchReadyView::SessionMissing => {
                // The session vanished between dispatch and launch. Resolve any
                // open reconnect busy so its spinner doesn't linger (a create
                // launch never reaches SessionMissing — it commits unconditionally
                // — so only the reconnect op needs clearing here), then clear a
                // still-showing anon launch busy as a final fallback.
                if let Some(op) = self.pending_reconnect_ops.remove(&outcome.session.id) {
                    self.apply_reaction(op.resolve(&TuiReconnectOutcome::Missing).into_reaction());
                }
                if matches!(self.status.most_recent_tui(), Some((StatusTone::Busy, _))) {
                    self.set_info(String::new());
                }
            }
            AgentLaunchReadyView::Reconnect { status_message } => {
                self.show_agent_surface();
                self.input_target = InputTarget::Agent;
                self.fullscreen_overlay = FullscreenOverlay::Agent;
                // Resolve the keyed reconnect op so its success replaces exactly
                // the "Launching…"/"Starting fresh…" busy. Falls back to an
                // anonymous info when no op is stashed (e.g. a launch not driven
                // through the reconnect dispatch sites).
                self.resolve_reconnect_op_or(
                    &outcome.session.id,
                    TuiReconnectOutcome::Ready { status_message },
                );
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
                self.resolve_reconnect_op_or(
                    &session_id,
                    TuiReconnectOutcome::Ready { status_message },
                );
            }
            AgentLaunchReadyView::StartupAutoReopen => {}
        }
    }

    fn apply_agent_launch_failed_view(&mut self, outcome: AgentLaunchFailedOutcome) {
        match outcome {
            AgentLaunchFailedOutcome::Create { .. } => {
                // The create op's keyed error final is resolved ENGINE-SIDE and
                // arrives as a sibling `Status` in the same `Multi`, so this arm
                // has no status to set.
            }
            AgentLaunchFailedOutcome::Reconnect {
                session_id,
                branch_name,
                message,
            } => {
                // Resolve the keyed reconnect op so its error replaces exactly the
                // "Launching…" busy; fall back to an anonymous error when no op is
                // stashed (the message is byte-identical either way).
                self.resolve_reconnect_op_or(
                    &session_id,
                    TuiReconnectOutcome::ReconnectFailed {
                        branch_name,
                        message,
                    },
                );
            }
            AgentLaunchFailedOutcome::ForceReconnect {
                session_id,
                branch_name,
                message,
            } => {
                self.resolve_reconnect_op_or(
                    &session_id,
                    TuiReconnectOutcome::ForceReconnectFailed {
                        branch_name,
                        message,
                    },
                );
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

    /// Resolve a stashed reconnect/fresh-restart [`HandlerStatusOp`] (keyed by
    /// session id) against `outcome`, replacing exactly its keyed busy. When no op
    /// is stashed (a launch ready/failed not driven through the reconnect dispatch
    /// sites), fall back to applying the SAME final anonymously via the shared
    /// [`super::reconnect_final`] mapping, so the wording is byte-identical to the
    /// pre-op behavior.
    fn resolve_reconnect_op_or(&mut self, session_id: &str, outcome: TuiReconnectOutcome) {
        if let Some(op) = self.pending_reconnect_ops.remove(session_id) {
            self.apply_reaction(op.resolve(&outcome).into_reaction());
            return;
        }
        // No op stashed: apply the SAME final anonymously (no key), preserving the
        // pre-op behavior. `reconnect_final` is the single wording source.
        match super::reconnect_final(&outcome) {
            dux_core::engine::Final::Message { tone, text } => {
                self.status.set(std::time::Instant::now(), None, tone, text);
            }
            dux_core::engine::Final::Clear => {
                if matches!(self.status.most_recent_tui(), Some((StatusTone::Busy, _))) {
                    self.set_info(String::new());
                }
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
    status_op_id: Option<String>,
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
    let _ = worker_tx.send(WorkerEvent::CreateAgentBranchInspected {
        project,
        result,
        status_op_id,
    });
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

    /// The create launch final (success / startup-error / persist-fail / launch-
    /// fail) is now resolved ENGINE-SIDE against the shared `pending_create_ops`
    /// op and arrives as a sibling keyed `Status` in the same `Multi` as the launch
    /// View; the TUI's `CreateCommitted` view arm only does the non-status work
    /// (rebuild/select/show surface) and sets NO status. The engine-side
    /// resolution is covered in `engine::events` tests.
    #[test]
    fn create_committed_view_sets_no_status_on_the_tui() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            session,
            pty_size: (80, 24),
            detached_session_id: None,
            view: AgentLaunchReadyView::CreateCommitted {
                status_message: "Created agent.".to_string(),
                startup_result_error: None,
            },
        });

        assert!(
            app.status.snapshot().is_empty(),
            "the create View arm must not set any status; the engine emits the keyed final",
        );
    }

    /// A reconnect success must resolve the keyed reconnect op in place: the
    /// op's pending Busy entry becomes a same-key Info final carrying the exact
    /// engine-computed status message, and the op is consumed.
    #[test]
    fn reconnect_ready_resolves_the_keyed_reconnect_op() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        // Mirror the dispatch site: mint the op, show its pending busy, stash it.
        let op = app
            .build_reconnect_status_op(format!("Launching agent \"{}\"...", session.branch_name));
        let op_key = op.id().to_string();
        app.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
        app.pending_reconnect_ops.insert(session.id.clone(), op);

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".to_string(),
            },
        });

        let entry = app
            .status
            .snapshot()
            .into_iter()
            .find(|s| s.key.as_deref() == Some(op_key.as_str()));
        let entry = entry.expect("the op's keyed entry must still exist, replaced in place");
        assert_eq!(entry.tone.as_str(), "info");
        assert_eq!(entry.message, "Reconnected.");
        assert!(
            app.pending_reconnect_ops.is_empty(),
            "the reconnect op must be consumed on resolution",
        );
    }

    /// A reconnect FAILURE resolves the same op to a same-key Error final whose
    /// wording is byte-identical to the legacy anonymous error.
    #[test]
    fn reconnect_failed_resolves_the_keyed_reconnect_op() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        let op = app
            .build_reconnect_status_op(format!("Launching agent \"{}\"...", session.branch_name));
        let op_key = op.id().to_string();
        app.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
        app.pending_reconnect_ops.insert(session.id.clone(), op);

        app.apply_agent_launch_failed_view(AgentLaunchFailedOutcome::Reconnect {
            session_id: session.id.clone(),
            branch_name: "feat".to_string(),
            message: "nope".to_string(),
        });

        let entry = app
            .status
            .snapshot()
            .into_iter()
            .find(|s| s.key.as_deref() == Some(op_key.as_str()))
            .expect("the op's keyed entry must still exist, replaced in place");
        assert_eq!(entry.tone.as_str(), "error");
        assert_eq!(entry.message, "Reconnect failed for agent \"feat\": nope");
        assert!(app.pending_reconnect_ops.is_empty());
    }

    /// When no reconnect op is stashed, the ready/failed handlers fall back to an
    /// ANONYMOUS final with byte-identical wording, preserving pre-op behavior.
    #[test]
    fn reconnect_without_op_falls_back_to_anonymous_final() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        app.apply_agent_launch_failed_view(AgentLaunchFailedOutcome::ForceReconnect {
            session_id: session.id.clone(),
            branch_name: "feat".to_string(),
            message: "boom".to_string(),
        });

        assert_eq!(
            app.status.message(),
            "Fresh restart failed for agent \"feat\": boom"
        );
        // No keyed entry was created for the anonymous fallback.
        assert!(
            app.status.snapshot().iter().all(|s| s.key.is_none()
                || s.tone.as_str() != "error"
                || s.message != "Fresh restart failed for agent \"feat\": boom"),
            "fallback must be anonymous (no key)",
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
            "op-test".to_string(),
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
            "op-create-1".to_string(),
        );

        match worker_rx.recv().expect("worker event") {
            WorkerEvent::CreateAgentProgress {
                status_op_id,
                message,
            } => {
                // The progress carries the opaque op id passed into the job, not a
                // content-addressable create:{project_id} key.
                assert_eq!(status_op_id, "op-create-1");
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
