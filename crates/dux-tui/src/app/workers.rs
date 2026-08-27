use std::sync::mpsc::Sender;

use dux_core::engine::{
    AgentLaunchFailedOutcome, AgentLaunchReadyOutcome, AgentLaunchReadyView,
    BeginDeleteSessionOutcome, BeginDeleteSessionView, DeleteTerminalView, DispatchAgentLaunchView,
    DoDeleteSessionView, EventReaction, FinishDeleteSessionView, ProjectPersistenceOutcome,
    ProjectPersistenceView, PrunedPty, PrunedPtyKind, StatusUpdate, WorktreeRemoval,
};

use super::*;

impl PruneViewContext {
    fn capture(app: &App) -> Self {
        let selected_session = app.selected_session().map(|session| session.id.clone());
        let focused_tab = selected_session
            .as_ref()
            .map(|session_id| app.focused_tab_id(session_id));
        let tab_providers = app
            .engine
            .agent_tabs
            .iter()
            .map(|(id, tab)| (id.clone(), tab.provider.as_str().to_string()))
            .collect();
        Self {
            selected_session,
            focused_tab,
            tab_providers,
        }
    }
}

impl DrainedEventMetadata {
    fn capture(event: &WorkerEvent) -> Self {
        Self {
            pr_lookup_completion: match event {
                WorkerEvent::PullRequestResolved {
                    status_op_id: Some(id),
                    result,
                    purpose: dux_core::worker::PrLookupPurpose::CreateAgent,
                } => Some((id.clone(), result.is_ok())),
                _ => None,
            },
            checkout_inspect_completion: match event {
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
                }
                | WorkerEvent::InitialCommitCreated {
                    status_op_id: Some(id),
                    ..
                } => Some(id.clone()),
                _ => None,
            },
            reference_resolution: match event {
                WorkerEvent::PullRequestReferenceResolved {
                    raw_input,
                    repository,
                    result,
                    status_op_id,
                } => Some(PrReferenceResolutionAnswer {
                    raw_input: raw_input.clone(),
                    repository: repository.clone(),
                    result: result.clone(),
                    status_op_id: status_op_id.clone(),
                }),
                _ => None,
            },
            changed_files_answer: match event {
                WorkerEvent::ChangedFilesReady { outcome, worktree } => Some(ChangedFilesAnswer {
                    worktree: worktree.clone(),
                    error: outcome.as_ref().err().cloned(),
                }),
                _ => None,
            },
        }
    }
}

impl App {
    pub(crate) fn drain_events(&mut self) {
        // The release-notes worker's PAYLOAD rides its own channel (the keyed
        // busy→final status rides the engine channel below as a
        // `StatusOpCompleted`), so fold it in on the same tick.
        self.drain_notes_fetch();
        self.drain_worker_events();
        self.apply_resume_fallback_sweep();
        self.dispatch_reaped_worktree_removals();
        let maintenance = self.apply_pruned_pty_events();
        self.note_companion_maintenance(&maintenance);
        self.refresh_resource_monitor_if_due();
        self.engine.sync_has_active_processes();
    }

    fn apply_pruned_pty_events(&mut self) -> dux_core::background_serve::DrainedMaintenance {
        let context = PruneViewContext::capture(self);
        let pruned = self.engine.prune_exited_ptys();
        let reported_prunes = if self.background_server_is_serving() {
            pruned.clone()
        } else {
            Vec::new()
        };
        self.apply_pruned_agent_tabs(&pruned, &context);
        self.apply_selected_agent_exit(&pruned);
        self.apply_pruned_terminals(&pruned);
        dux_core::background_serve::DrainedMaintenance {
            pruned: reported_prunes,
            foregrounds_changed: self.engine.refresh_terminal_foregrounds(),
        }
    }

    fn refresh_resource_monitor_if_due(&mut self) {
        if let PromptState::ResourceMonitor {
            ref last_refresh, ..
        } = self.prompt
            && last_refresh.elapsed() >= Duration::from_secs(2)
        {
            self.engine.spawn_resource_stats_worker();
        }
    }

    fn apply_pruned_agent_tabs(&mut self, pruned: &[PrunedPty], context: &PruneViewContext) {
        let mut rebuild_needed = false;
        for pty in pruned.iter().filter(|pty| pty.kind == PrunedPtyKind::Agent) {
            rebuild_needed |= self.apply_pruned_agent_tab(pty, context);
        }
        if rebuild_needed {
            self.rebuild_left_items();
        }
    }

    fn apply_pruned_agent_tab(&mut self, pty: &PrunedPty, context: &PruneViewContext) -> bool {
        let Some(session_id) = session_owner_id(pty) else {
            return false;
        };
        let was_focused_tab = context.selected_session.as_deref() == Some(session_id)
            && context.focused_tab.as_deref() == Some(pty.id.as_str());
        let support_provider = (pty.id != session_id).then(|| {
            context
                .tab_providers
                .get(&pty.id)
                .cloned()
                .unwrap_or_default()
        });
        if pty.tab_closed {
            self.apply_closed_agent_tab(pty, session_id, support_provider, was_focused_tab);
            true
        } else {
            self.apply_exited_agent_tab(support_provider, was_focused_tab);
            false
        }
    }

    fn apply_closed_agent_tab(
        &mut self,
        pty: &PrunedPty,
        session_id: &str,
        support_provider: Option<String>,
        was_focused_tab: bool,
    ) {
        if let Some(provider) = support_provider {
            self.set_info(format!("Tab ({provider}) exited cleanly and was closed."));
        }
        if !was_focused_tab {
            return;
        }
        let target = self
            .engine
            .first_live_tab(session_id)
            .unwrap_or_else(|| session_id.to_string());
        self.set_focused_tab(session_id, &target);
        if self.session_surface == SessionSurface::Agent {
            self.reset_raw_input_state();
            self.fullscreen_overlay = FullscreenOverlay::None;
            if pty.agent_detached {
                self.focus = FocusPane::Left;
            }
        }
    }

    fn apply_exited_agent_tab(&mut self, support_provider: Option<String>, was_focused_tab: bool) {
        if let Some(provider) = support_provider {
            self.set_info(format!("Tab ({provider}) exited."));
        }
        if self.input_target == InputTarget::Agent
            && self.session_surface == SessionSurface::Agent
            && was_focused_tab
        {
            self.reset_raw_input_state();
        }
    }

    fn apply_selected_agent_exit(&mut self, pruned: &[PrunedPty]) {
        let Some(current_id) = self.selected_session().map(|session| session.id.clone()) else {
            return;
        };
        let Some(pty) = pruned
            .iter()
            .find(|pty| is_session_slot_prune(pty, &current_id))
        else {
            return;
        };
        let focused = self.focused_tab_id(&current_id);
        if focused != current_id && self.engine.providers.contains_key(&focused) {
            return;
        }
        self.apply_selected_agent_exit_status(pty);
    }

    fn apply_selected_agent_exit_status(&mut self, pty: &PrunedPty) {
        let key = self.bindings.label_for(Action::ReconnectAgent);
        if self.session_surface != SessionSurface::Agent {
            self.set_info(format!(
                "Agent CLI process exited. Companion terminal is still available; press \"{key}\" to relaunch the agent."
            ));
            return;
        }
        self.log_minimal_agent_exit(pty);
        let status =
            agent_exit_status_message(pty.exit_success, pty.is_minimal, &pty.output_excerpt, &key);
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.focus = FocusPane::Left;
        if pty.exit_success == Some(false) {
            self.set_error(status);
        } else {
            self.set_info(status);
        }
    }

    fn log_minimal_agent_exit(&self, pty: &PrunedPty) {
        if !pty.is_minimal || pty.output_excerpt.trim().is_empty() {
            return;
        }
        if let Some(current) = self.selected_session() {
            let branch = current.display_label();
            let provider = self
                .engine
                .running_provider_for(current)
                .as_str()
                .to_string();
            logger::error(&format!(
                "Agent CLI process for agent \"{branch}\" ({provider}) exited. Full captured output:\n{}",
                pty.output_excerpt
            ));
        }
    }

    fn apply_pruned_terminals(&mut self, pruned: &[PrunedPty]) {
        let exited_terminal_ids: Vec<&str> = pruned
            .iter()
            .filter(|pty| pty.kind == PrunedPtyKind::Terminal)
            .map(|pty| pty.id.as_str())
            .collect();
        if exited_terminal_ids.is_empty() {
            return;
        }
        if let Some(active_id) = self.active_terminal_id.as_deref()
            && exited_terminal_ids.contains(&active_id)
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
        self.rebuild_left_items();
    }

    fn drain_worker_events(&mut self) {
        while let Ok(event) = self.engine.worker_rx.try_recv() {
            let metadata = DrainedEventMetadata::capture(&event);
            self.disarm_tui_launch_for_failed_event(&event);
            let reaction = self.engine.process_worker_event(event);
            let chains_forward = matches!(
                reaction,
                EventReaction::DispatchProjectDefaultBranchCheckout { .. }
            );
            let routing = self.companion_routing();
            self.notify_companion(&reaction);
            self.apply_routed_reaction(reaction, &routing);
            self.apply_drained_event_metadata(metadata, chains_forward);
        }
    }

    fn apply_resume_fallback_sweep(&mut self) {
        let sweep_size = self.pty_size_for_launch();
        for reaction in self.engine.sweep_resume_fallbacks(sweep_size) {
            let routing = self.companion_routing();
            self.notify_companion(&reaction);
            self.apply_routed_reaction(reaction, &routing);
        }
    }

    fn dispatch_reaped_worktree_removals(&mut self) {
        for removal in self.engine.reap_terminating_ptys() {
            let _busy = self.engine.dispatch_deferred_worktree_removal(removal);
        }
    }

    fn disarm_tui_launch_for_failed_event(&mut self, event: &WorkerEvent) {
        match event {
            WorkerEvent::AgentLaunchFailed(data) => {
                self.tui_launched_ptys.remove(&data.request.tab_id);
                if matches!(data.request.kind, AgentLaunchKind::Create { .. }) {
                    self.create_agent_started_here = false;
                }
            }
            WorkerEvent::CreateAgentFailed { .. } => {
                self.create_agent_started_here = false;
            }
            _ => {}
        }
    }

    fn apply_drained_event_metadata(
        &mut self,
        metadata: DrainedEventMetadata,
        chains_forward: bool,
    ) {
        if let Some(answer) = metadata.changed_files_answer {
            self.apply_changed_files_refresh_outcome(&answer.worktree, answer.error);
        }
        if let Some(answer) = metadata.reference_resolution {
            self.apply_pr_reference_resolution_answer(answer);
        }
        if let Some(completion) = metadata.pr_lookup_completion {
            self.resolve_pr_lookup_completion(completion);
        }
        if let Some(id) = metadata.checkout_inspect_completion {
            self.resolve_checkout_inspect_completion(id, chains_forward);
        }
    }

    fn apply_pr_reference_resolution_answer(&mut self, answer: PrReferenceResolutionAnswer) {
        let current = self.pending_pr_reference_op.as_deref() == answer.status_op_id.as_deref()
            && answer.status_op_id.is_some();
        if current {
            self.pending_pr_reference_op = None;
            if let Err(error) = self.apply_pull_request_reference_resolution(
                answer.raw_input,
                answer.repository,
                answer.result,
            ) {
                self.set_error(format!("{error:#}"));
            }
        }
        if let Some(id) = answer.status_op_id
            && let Some(op) = self.pending_pr_lookup_ops.remove(&id)
        {
            self.apply_reaction(op.resolve(&PrLookupFinalOutcome::HandedOff).into_reaction());
        }
    }

    fn resolve_pr_lookup_completion(&mut self, completion: (String, bool)) {
        let (id, succeeded) = completion;
        if let Some(op) = self.pending_pr_lookup_ops.remove(&id) {
            let outcome = if succeeded {
                PrLookupFinalOutcome::HandedOff
            } else {
                PrLookupFinalOutcome::Failed
            };
            self.apply_reaction(op.resolve(&outcome).into_reaction());
        }
    }

    fn resolve_checkout_inspect_completion(&mut self, id: String, chains_forward: bool) {
        if !chains_forward && let Some(op) = self.pending_checkout_inspect_ops.remove(&id) {
            self.apply_reaction(op.resolve(&TuiCheckoutInspectOutcome::Done).into_reaction());
        }
    }

    /// Apply a reaction this surface minted itself, or one nothing else has had a
    /// chance to fan out yet.
    ///
    /// Routes against the LIVE pending-op maps, which is the right answer for
    /// every caller outside the drain: a reaction built here and applied here
    /// cannot have had its op consumed in between. The drain takes its verdict
    /// first and calls [`Self::apply_routed_reaction`] instead.
    pub(super) fn apply_reaction(&mut self, reaction: EventReaction) {
        let routing = self.companion_routing();
        self.apply_routed_reaction(reaction, &routing);
    }

    pub(super) fn apply_routed_reaction(
        &mut self,
        reaction: EventReaction,
        routing: &CompanionRouting,
    ) {
        // ORIGIN ROUTING. While the background web server is on, both surfaces see
        // the same worker events, and a few reactions carry a follow-up that DOES
        // something: it spawns a git job, adds a project, or dispatches an agent
        // create. Those belong to whichever surface asked for them, and the engine
        // is the one that knows (see `dux_core::engine::owner_of_reaction`). A
        // browser's PR-create must not also pop a name prompt here.
        //
        // Checked here rather than per arm so a `Multi` routes leaf by leaf: this
        // function is what recurses through one, carrying the same verdict source
        // down so a leaf cannot be judged against a map the fanout has since
        // emptied.
        if routing.companion_owns(&reaction) {
            // The web's follow-up for this ran during the fanout and can have
            // mutated the workspace (the inline project add writes
            // `engine.projects` from in there), so this surface has to re-derive
            // its view even though it did no work itself. `service_companion`
            // folds this into its own post-mutation refresh at the end of the
            // iteration.
            self.companion_followup_ran = true;
            return;
        }
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
                // The SAME verdict source for every leaf: a `Multi` routes leaf by
                // leaf, and re-deriving it here would read maps the companion's
                // fanout may already have emptied.
                for r in reactions {
                    self.apply_routed_reaction(r, routing);
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

            EventReaction::BrowserEntriesArrived { dir, entries } => {
                self.apply_browser_entries(dir, entries);
            }
            EventReaction::ProjectWorktreesArrived {
                project_id,
                result,
                status_op_id,
            } => {
                self.apply_project_worktrees_arrived(project_id, result, status_op_id);
            }

            EventReaction::ManageableWorktreesArrived {
                project_id,
                result,
                status_op_id,
            } => {
                self.apply_manageable_worktrees_arrived(project_id, result, status_op_id);
            }

            EventReaction::OpenNewAgentPromptForPr {
                pr,
                status_op_id: _,
            } => {
                self.apply_open_new_agent_prompt_for_pr(*pr);
            }
            EventReaction::WorktreeRemoveSucceeded {
                session_id,
                branches,
                our_busy_message: _,
            } => {
                self.apply_worktree_remove_succeeded(session_id, branches);
            }
            EventReaction::WorktreeRemoveFailed {
                session_id,
                message,
            } => {
                self.apply_worktree_remove_failed(session_id, message);
            }

            EventReaction::ResourceStatsArrived(stats, was_baseline) => {
                self.apply_resource_stats(stats, was_baseline);
            }

            EventReaction::AddProjectAfterBranchCheckout {
                path,
                name,
                target_branch,
                leading_branch,
                status_op_id: _,
            } => {
                self.apply_add_project_after_branch_checkout(
                    path,
                    name,
                    target_branch,
                    leading_branch,
                );
            }

            EventReaction::AddProjectAfterInitialCommit {
                path,
                name,
                branch,
                leading_branch,
                initialized_repo,
                seeded_gitignore,
                seed_warning,
                status_op_id: _,
            } => {
                self.apply_add_project_after_initial_commit(
                    path,
                    name,
                    branch,
                    leading_branch,
                    (initialized_repo, seeded_gitignore, seed_warning),
                );
            }

            EventReaction::ContinueCreateAgentAfterInspection {
                project,
                inspection,
            } => {
                self.apply_continue_create_agent_after_inspection(project, inspection);
            }
            EventReaction::DispatchProjectDefaultBranchCheckout {
                project,
                default_branch,
                status_op_id,
            } => {
                self.apply_dispatch_project_default_branch_checkout(
                    project,
                    default_branch,
                    status_op_id,
                );
            }

            EventReaction::TailscaleModeApplied { mode, outcome } => {
                self.apply_tailscale_mode_outcome(mode, outcome);
            }
            EventReaction::ApplyReloadedConfig(boxed) => {
                self.apply_reloaded_config_reaction(*boxed);
            }
            EventReaction::OpenConfigReloadFailedModal(message) => {
                self.apply_open_config_reload_failed_modal(message);
            }

            EventReaction::ProjectPersistenceOutcome(boxed) => {
                self.apply_project_persistence_outcome(*boxed);
            }

            EventReaction::StartupLogsArrived {
                scope_label,
                listing,
            } => {
                self.apply_startup_logs_arrived(scope_label, listing);
            }

            EventReaction::StartupLogContentArrived { path, result } => {
                self.apply_startup_command_log_content(&path, result);
            }

            EventReaction::FinishDeleteSessionView(view) => {
                self.apply_finish_delete_session_view(*view);
            }

            EventReaction::DoDeleteSessionView(view) => {
                self.apply_do_delete_session_view(*view);
            }

            EventReaction::BeginDeleteSessionView(view) => {
                self.apply_begin_delete_session_view(*view);
            }

            EventReaction::DispatchAgentLaunchView(view) => {
                self.apply_dispatch_agent_launch_view(*view);
            }

            EventReaction::DeleteTerminalView(view) => {
                self.apply_delete_terminal_view(*view);
            }

            EventReaction::ServerFlipPreflightReady { result, warning } => {
                self.apply_server_flip_preflight(result, warning);
            }

            EventReaction::BackgroundServerPreflightReady { result, warning } => {
                self.apply_background_server_preflight(result, warning);
            }
        }
    }

    fn apply_add_project_after_branch_checkout(
        &mut self,
        path: String,
        name: String,
        target_branch: String,
        leading_branch: String,
    ) {
        let display_name = project_display_name(&path, &name);
        let status_message = format!(
            "Checked out \"{target_branch}\" and added project \"{display_name}\" to workspace."
        );
        if let Err(error) = self.finish_add_project_with_status(
            path,
            name,
            target_branch.clone(),
            leading_branch,
            status_message,
        ) {
            self.set_error(format!("{error:#}"));
        }
    }

    fn apply_add_project_after_initial_commit(
        &mut self,
        path: String,
        name: String,
        branch: String,
        leading_branch: String,
        setup: (bool, bool, Option<String>),
    ) {
        let (initialized_repo, seeded_gitignore, seed_warning) = setup;
        let display_name = project_display_name(&path, &name);
        let status_message =
            initial_commit_project_status(&display_name, initialized_repo, seeded_gitignore);
        if let Err(error) =
            self.finish_add_project_with_status(path, name, branch, leading_branch, status_message)
        {
            self.set_error(format!("{error:#}"));
        }
        if let Some(warning) = seed_warning {
            self.set_warning(warning);
        }
    }

    fn apply_continue_create_agent_after_inspection(
        &mut self,
        project: Project,
        inspection: CreateAgentBranchInspection,
    ) {
        let project_name = project.name.clone();
        match self.sync_projects_to_store_and_update_config() {
            Ok(()) => {
                if let Err(error) = self
                    .engine
                    .config_writer
                    .save_eager(self.engine.config.clone())
                {
                    self.set_error(format!(
                        "Project branch was detected, but config.toml could not be updated: {error}"
                    ));
                }
            }
            Err(error) => self.set_error(format!(
                "Project branch was detected, but config.toml could not be updated: {error:#}"
            )),
        }
        if let Err(error) = self.continue_create_agent_after_branch_inspection(project, inspection)
        {
            self.set_error(format!("{error:#}"));
        } else {
            self.set_info(format!(
                "Branch check complete for \"{project_name}\". Confirm or edit the agent name to continue."
            ));
        }
    }

    fn apply_dispatch_project_default_branch_checkout(
        &mut self,
        project: Project,
        default_branch: String,
        status_op_id: Option<String>,
    ) {
        let action = NonDefaultBranchAction::CheckoutProjectDefault {
            project: project.clone(),
        };
        let path = action.repo_path().to_string();
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

    fn apply_reloaded_config_reaction(&mut self, config: Config) {
        let bind_settings_changed = dux_core::config::server_bind_settings_changed(
            &self.engine.config.server,
            &config.server,
        );
        let outcome = match self.apply_reloaded_config(config) {
            Err(error) => TuiConfigReloadOutcome::ApplyFailed(format!("{error:#}")),
            Ok(()) => TuiConfigReloadOutcome::Applied,
        };
        let applied = matches!(outcome, TuiConfigReloadOutcome::Applied);
        if applied && let Some(companion) = self.companion.as_mut() {
            companion.note_config_applied(&self.engine.config.server);
        }
        if let Some(op) = self.pending_config_reload_op.take() {
            self.apply_reaction(op.resolve(&outcome).into_reaction());
        } else {
            self.apply_unkeyed_config_reload_outcome(&outcome);
        }
        if applied && bind_settings_changed {
            let serving = self.background_server_is_serving();
            self.set_pinned_warning(server_restart_warning(serving));
        }
    }

    fn apply_unkeyed_config_reload_outcome(&mut self, outcome: &TuiConfigReloadOutcome) {
        match outcome {
            TuiConfigReloadOutcome::Applied => {
                self.set_info("Configuration reloaded. New settings are active now.");
            }
            TuiConfigReloadOutcome::ApplyFailed(error) => self.set_error(format!(
                "Config validation passed, but applying it failed: {error}"
            )),
            TuiConfigReloadOutcome::ValidationFailed => {}
        }
    }

    fn apply_open_config_reload_failed_modal(&mut self, message: String) {
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

    fn apply_server_flip_preflight(
        &mut self,
        result: Result<(Vec<std::net::TcpListener>, Vec<String>), String>,
        warning: Option<String>,
    ) {
        self.server_flip_preflight_pending = false;
        match result {
            Ok((listeners, urls)) => {
                self.apply_server_flip_preflight_success(listeners, urls, warning)
            }
            Err(error) => {
                if let Some(op) = self.pending_server_flip_op.take() {
                    self.apply_reaction(
                        op.resolve(&TuiServerFlipOutcome::Failed(error))
                            .into_reaction(),
                    );
                }
            }
        }
    }

    fn apply_server_flip_preflight_success(
        &mut self,
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
        warning: Option<String>,
    ) {
        let url_list = urls.join(", ");
        if let Some(warning) = warning {
            if let Some(op) = self.pending_server_flip_op.take() {
                self.apply_reaction(
                    op.resolve(&TuiServerFlipOutcome::Warned(format!(
                        "{warning} Starting the web server on {url_list} — your agents keep running."
                    )))
                    .into_reaction(),
                );
            }
        } else if let Some(op) = &self.pending_server_flip_op {
            let progress = op.progress(format!(
                "Starting the web server on {url_list} — your agents keep running."
            ));
            self.apply_reaction(EventReaction::Status(progress));
        }
        self.pending_server_flip = Some((listeners, urls));
    }

    fn apply_browser_entries(&mut self, dir: PathBuf, entries: Vec<BrowserEntry>) {
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

    fn apply_worktree_remove_succeeded(
        &mut self,
        session_id: String,
        branches: dux_core::engine::RemovedBranches,
    ) {
        let op = self.pending_delete_ops.remove(&session_id);
        if self.engine.sessions.iter().any(|s| s.id == session_id) {
            let cleanup = self.finish_delete_session(
                &session_id,
                WorktreeRemoval::Performed {
                    branches: branches.clone(),
                },
                false,
            );
            if let Err(error) = cleanup {
                self.set_error(format!(
                    "Worktree removed but session cleanup failed: {error:#}"
                ));
            } else if let Some(op) = op {
                self.apply_reaction(
                    op.resolve(&TuiDeleteOutcome::SucceededPresent { branches })
                        .into_reaction(),
                );
            }
            return;
        }
        if let Some(op) = op {
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

    fn apply_worktree_remove_failed(&mut self, session_id: String, message: String) {
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

    fn apply_begin_delete_session_view(&mut self, view: BeginDeleteSessionView) {
        let BeginDeleteSessionView {
            session_id,
            outcome,
        } = view;
        match outcome {
            BeginDeleteSessionOutcome::AlreadyInFlight => {
                self.set_error(
                    "Deletion already in progress for this agent. Wait for it to finish.",
                );
            }
            BeginDeleteSessionOutcome::TabLaunching => {
                self.set_error("A tab is still launching for this agent. Try again in a moment.");
            }
            BeginDeleteSessionOutcome::NotFound => {}
            BeginDeleteSessionOutcome::Refused { message } => self.set_error(message),
            BeginDeleteSessionOutcome::AsyncStarted { busy_message } => {
                let removal = WorktreeRemoval::Performed {
                    branches: dux_core::engine::RemovedBranches::Deleted(
                        dux_core::git::RemoveResult::default(),
                    ),
                };
                if let Err(error) = self.finish_delete_session(&session_id, removal, false) {
                    self.set_error(format!("Failed to delete agent: {error:#}"));
                } else {
                    let op = self.build_delete_status_op(&session_id, busy_message);
                    self.apply_reaction(EventReaction::Status(op.pending_status()));
                    self.pending_delete_ops.insert(session_id, op);
                }
            }
            BeginDeleteSessionOutcome::Inline { removal } => {
                if let Err(error) = self.finish_delete_session(&session_id, removal, true) {
                    self.set_error(format!("{error:#}"));
                }
            }
        }
    }

    fn apply_finish_delete_session_view(&mut self, view: FinishDeleteSessionView) {
        self.apply_finish_delete_session_outcome(
            &view.session_id,
            view.outcome,
            view.removal,
            view.update_status,
        );
    }

    fn apply_do_delete_session_view(&mut self, view: DoDeleteSessionView) {
        self.apply_finish_delete_session_outcome(
            &view.session_id,
            view.outcome.finish,
            view.outcome.removal,
            true,
        );
    }

    fn apply_startup_logs_arrived(
        &mut self,
        scope_label: String,
        listing: dux_core::startup::StartupCommandLogListing,
    ) {
        self.input_target = InputTarget::None;
        self.terminal_selection = None;
        self.startup_log_selection = None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.startup_log_viewer = None;
        self.prompt = PromptState::StartupCommandLogs(StartupCommandLogPrompt {
            scope_label,
            entries: listing.entries,
            selected: 0,
            filter: TextInput::new(),
            searching: false,
            content: listing.content,
            scroll_offset: 0,
            wrap_width: 0,
            focus: StartupCommandLogFocus::List,
        });
    }

    fn apply_dispatch_agent_launch_view(&mut self, view: DispatchAgentLaunchView) {
        if let Some(status) = view.status {
            self.apply_reaction(EventReaction::Status(status));
        }
    }

    fn apply_delete_terminal_view(&mut self, view: DeleteTerminalView) {
        if self.active_terminal_id.as_deref() == Some(view.terminal_id.as_str()) {
            self.active_terminal_id = None;
        }
        self.clamp_terminal_cursor();
        self.rebuild_left_items();
        if let Some(label) = view.label {
            self.set_info(format!("Deleted terminal \"{label}\""));
        }
    }

    fn apply_project_worktrees_arrived(
        &mut self,
        project_id: String,
        result: Result<Vec<ProjectWorktreeEntry>, String>,
        status_op_id: Option<String>,
    ) {
        let outcome = if let PromptState::PickProjectWorktree(prompt) = &mut self.prompt
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
                    WorktreesFinalOutcome::Loaded
                }
                Err(error) => {
                    prompt.entries.clear();
                    prompt.selected = None;
                    prompt.error = Some(error.clone());
                    WorktreesFinalOutcome::Failed(error)
                }
            }
        } else {
            WorktreesFinalOutcome::Dismissed
        };
        self.resolve_worktree_listing_op(status_op_id, outcome);
    }

    fn apply_manageable_worktrees_arrived(
        &mut self,
        project_id: String,
        result: Result<Vec<dux_core::worktree_manager::ManagedWorktree>, String>,
        status_op_id: Option<String>,
    ) {
        let outcome = if let PromptState::ManageWorktrees(prompt) = &mut self.prompt
            && prompt.project.id == project_id
        {
            prompt.loading = false;
            match result {
                Ok(entries) => {
                    prompt.selected = removable_worktree_indices(&entries).into_iter().next();
                    prompt.entries = entries;
                    prompt.error = None;
                    WorktreesFinalOutcome::Loaded
                }
                Err(error) => {
                    prompt.entries.clear();
                    prompt.selected = None;
                    prompt.error = Some(error.clone());
                    WorktreesFinalOutcome::Failed(error)
                }
            }
        } else {
            WorktreesFinalOutcome::Dismissed
        };
        self.resolve_worktree_listing_op(status_op_id, outcome);
    }

    fn resolve_worktree_listing_op(
        &mut self,
        status_op_id: Option<String>,
        outcome: WorktreesFinalOutcome,
    ) {
        if let Some(id) = status_op_id
            && let Some(op) = self.pending_worktree_ops.remove(&id)
        {
            self.apply_reaction(op.resolve(&outcome).into_reaction());
        }
    }

    fn apply_open_new_agent_prompt_for_pr(&mut self, pr: dux_core::worker::ResolvedPullRequest) {
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

    fn apply_resource_stats(&mut self, stats: Vec<ResourceStats>, was_baseline: bool) {
        if let PromptState::ResourceMonitor {
            rows,
            selected_row,
            expanded,
            last_refresh,
            short_window_sample,
            ..
        } = &mut self.prompt
        {
            *rows = stats;
            *last_refresh = Instant::now();
            *short_window_sample = was_baseline;
            let max_row = build_visual_rows(rows, expanded).len().saturating_sub(1);
            if *selected_row > max_row {
                *selected_row = max_row;
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

    fn apply_project_persistence_failure(
        &mut self,
        action: ProjectPersistenceAction,
        error: String,
        status_op_id: &Option<String>,
    ) {
        if self.resolve_persist_op(status_op_id, PersistFinalOutcome::DbFailed(error.clone())) {
            return;
        }
        self.set_error(project_persistence_failure_message(action, &error));
    }

    fn apply_added_project(&mut self, status_message: String) {
        self.rebuild_left_items();
        self.reload_changed_files();
        // Add already saves config inline with database rollback; saving here would write twice.
        self.set_info(status_message);
    }

    fn apply_project_removal(
        &mut self,
        status_op_id: &Option<String>,
        config_error_prefix: &'static str,
        success_message: String,
    ) {
        self.rebuild_left_items();
        self.ensure_selectable_left_item();
        self.reload_changed_files();
        self.save_runtime_projects_and_finish_status(
            status_op_id,
            config_error_prefix,
            success_message,
        );
    }

    fn apply_default_provider_update(
        &mut self,
        project_name: String,
        provider: Option<ProviderKind>,
        global_default: ProviderKind,
        status_op_id: &Option<String>,
    ) {
        self.rebuild_left_items();
        let success_message = match provider {
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
        self.save_runtime_projects_and_finish_status(
            status_op_id,
            &format!(
                "Provider preference saved to the database for \"{project_name}\", but config.toml could not be updated"
            ),
            success_message,
        );
    }

    fn apply_auto_reopen_update(
        &mut self,
        project_name: String,
        auto_reopen_agents: Option<bool>,
        status_op_id: &Option<String>,
    ) {
        let enabled = auto_reopen_agents.unwrap_or(true);
        self.save_runtime_projects_and_finish_status(
            status_op_id,
            &format!(
                "Auto-reopen preference saved to the database for \"{project_name}\", but config.toml could not be updated"
            ),
            format!(
                "Startup auto-reopen {} for project \"{}\".",
                if enabled { "enabled" } else { "disabled" },
                project_name,
            ),
        );
    }

    fn apply_startup_command_update(
        &mut self,
        project_name: String,
        startup_command: Option<String>,
        status_op_id: &Option<String>,
    ) {
        let success_message = match startup_command {
            Some(command) => {
                format!("Startup command for project \"{project_name}\" set to: {command}")
            }
            None => format!("Startup command cleared for project \"{project_name}\"."),
        };
        self.save_runtime_projects_and_finish_status(
            status_op_id,
            &format!(
                "Startup command saved to the database for \"{project_name}\", but config.toml could not be updated"
            ),
            success_message,
        );
    }

    fn apply_env_update(
        &mut self,
        project_name: String,
        env_count: usize,
        status_op_id: &Option<String>,
    ) {
        let success_message = if env_count == 0 {
            format!("Environment variables cleared for project \"{project_name}\".")
        } else {
            format!(
                "Saved {env_count} environment variable(s) for project \"{project_name}\". New agents and terminals will receive them."
            )
        };
        self.save_runtime_projects_and_finish_status(
            status_op_id,
            &format!(
                "Environment variables saved to the database for \"{project_name}\", but config.toml could not be updated"
            ),
            success_message,
        );
    }

    fn save_runtime_projects_and_finish_status(
        &mut self,
        status_op_id: &Option<String>,
        config_error_prefix: &str,
        success_message: String,
    ) {
        self.update_config_projects_from_runtime();
        match self
            .engine
            .config_writer
            .save_eager(self.engine.config.clone())
        {
            Ok(()) => {
                if !self.resolve_persist_op(status_op_id, PersistFinalOutcome::Saved) {
                    self.set_info(success_message);
                }
            }
            Err(error) => {
                let error = error.to_string();
                if !self.resolve_persist_op(
                    status_op_id,
                    PersistFinalOutcome::ConfigWriteFailed(error.clone()),
                ) {
                    self.set_error(format!("{config_error_prefix}: {error}"));
                }
            }
        }
    }

    pub(crate) fn apply_project_persistence_outcome(&mut self, outcome: ProjectPersistenceOutcome) {
        let ProjectPersistenceOutcome {
            action,
            view,
            status_op_id,
        } = outcome;

        match view {
            ProjectPersistenceView::PersistenceFailed { error } => {
                self.apply_project_persistence_failure(action, error, &status_op_id);
            }
            ProjectPersistenceView::Added {
                project_id: _,
                status_message,
            } => {
                self.apply_added_project(status_message);
            }
            ProjectPersistenceView::Removed { project_name } => {
                self.apply_project_removal(
                    &status_op_id,
                    "Project was removed from the database, but config.toml could not be updated",
                    format!("Removed project \"{project_name}\" from app"),
                );
            }
            ProjectPersistenceView::Deleted { project_name } => {
                self.apply_project_removal(
                    &status_op_id,
                    "Project was deleted from the database, but config.toml could not be updated",
                    format!("Deleted project \"{project_name}\" and all its agents"),
                );
            }
            ProjectPersistenceView::DefaultProviderUpdated {
                project_name,
                provider,
                global_default,
            } => {
                self.apply_default_provider_update(
                    project_name,
                    provider,
                    global_default,
                    &status_op_id,
                );
            }
            ProjectPersistenceView::AutoReopenUpdated {
                project_name,
                auto_reopen_agents,
            } => {
                self.apply_auto_reopen_update(project_name, auto_reopen_agents, &status_op_id);
            }
            ProjectPersistenceView::StartupCommandUpdated {
                project_name,
                startup_command,
            } => {
                self.apply_startup_command_update(project_name, startup_command, &status_op_id);
            }
            ProjectPersistenceView::EnvUpdated {
                project_name,
                env_count,
            } => {
                self.apply_env_update(project_name, env_count, &status_op_id);
            }
        }
    }

    pub(super) fn apply_agent_launch_ready_view(&mut self, outcome: AgentLaunchReadyOutcome) {
        self.last_pty_size = outcome.pty_size;
        // OWNERSHIP OF A CHILD THIS SURFACE STARTED. This arm runs on both
        // surfaces (see `owner_of_reaction`), so the launch has to be recognised
        // as this one's rather than assumed: an id armed at dispatch for every
        // ordinary launch, and the create flag for a create, whose session id is
        // minted in the worker and so cannot be armed by id at all.
        //
        // Both are spent whether or not the claim happens, and the claim is made
        // only for a launch that really produced a child: a claim recorded
        // against an id no pty answers to is a driver nobody can see or take
        // over.
        let armed = self.tui_launched_ptys.remove(&outcome.tab_id);
        let created_here = matches!(
            &outcome.view,
            AgentLaunchReadyView::CreateCommitted { .. }
                | AgentLaunchReadyView::CreatePersistFailed { .. }
        ) && std::mem::take(&mut self.create_agent_started_here);
        if (armed || created_here) && self.engine.providers.contains_key(&outcome.tab_id) {
            self.claim_launched_pty(&outcome.tab_id);
        }
        // The engine's `detach_conflicting_worktree_session` already cleared every
        // runtime map (incl. pty_activity/pty_input) for the detached agent's
        // tabs, so no follow-up clear is needed here.
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
                // The selection just moved onto the freshly created agent, so a
                // lingering `manage-projects` target no longer matches what the
                // cursor points at; clear it so a follow-up project action
                // resolves the new agent's project, not the stale pick.
                self.project_chooser_context = None;
                self.reload_changed_files();
                self.show_agent_surface();
                // A launched agent lands focused-but-minimized
                // (Center focused, typeable); only a fullscreen-seeking launch
                // lands fullscreen. A create is never fullscreen-seeking, but
                // the shared landing helper keeps the rule in one place.
                self.land_completed_launch(outcome.wants_fullscreen);
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
                    self.apply_reaction(
                        op.resolve(&dux_core::engine::LaunchOutcome::Missing)
                            .into_reaction(),
                    );
                }
                if matches!(self.status.most_recent_tui(), Some((StatusTone::Busy, _))) {
                    self.set_info(String::new());
                }
            }
            AgentLaunchReadyView::Reconnect { status_message } => {
                self.show_agent_surface();
                // Land minimized unless the launch sought
                // fullscreen (see CreateCommitted above).
                self.land_completed_launch(outcome.wants_fullscreen);
                // Resolve the keyed reconnect op so its success replaces exactly
                // the "Launching…"/"Starting fresh…" busy. Falls back to an
                // anonymous info when no op is stashed (e.g. a launch not driven
                // through the reconnect dispatch sites).
                // Key by tab id: the session-slot tab's == its session id (resolves its
                // pending reconnect op as before), but an extra-tab launch has no
                // op under its tab id, so it falls through to an anonymous status
                // instead of resolving the session-slot tab's op with the wrong message.
                // The engine's message is shared with the web; the TUI appends
                // where the launch landed and how to toggle fullscreen.
                let status_message =
                    self.launch_completion_message(status_message, outcome.wants_fullscreen);
                self.resolve_reconnect_op_or(
                    &outcome.tab_id,
                    dux_core::engine::LaunchOutcome::Ready { status_message },
                );
                // The engine flipped the session Active while launching it, so the
                // flat list must re-partition: a just-reconnected agent leaves the
                // Inactive tail and rejoins the active section. Re-follow it by id
                // so the cursor stays on the agent as its row moves.
                self.rebuild_left_items();
                self.reselect_left_session(&outcome.session.id);
            }
            AgentLaunchReadyView::ResumeFallback {
                session_id,
                status_message,
            } => {
                let landed_here = self.selected_session().map(|selected| selected.id.as_str())
                    == Some(session_id.as_str());
                let status_message = if landed_here {
                    self.show_agent_surface();
                    // The fallback relaunch is engine-initiated
                    // and never fullscreen-seeking, so it lands minimized (see
                    // CreateCommitted above). The landing note is appended only
                    // when the landing actually happened: a fallback for an
                    // unselected agent moves no focus, so promising a typeable
                    // pane there would be a lie.
                    self.land_completed_launch(outcome.wants_fullscreen);
                    self.launch_completion_message(status_message, outcome.wants_fullscreen)
                } else {
                    status_message
                };
                self.resolve_reconnect_op_or(
                    &session_id,
                    dux_core::engine::LaunchOutcome::Ready { status_message },
                );
                // Same re-partition as Reconnect: the resumed agent is Active now.
                self.rebuild_left_items();
                self.reselect_left_session(&session_id);
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
                agent_label,
                message,
            } => {
                // Resolve the keyed reconnect op so its error replaces exactly the
                // "Launching…" busy; fall back to an anonymous error when no op is
                // stashed (the message is byte-identical either way).
                self.resolve_reconnect_op_or(
                    &session_id,
                    dux_core::engine::LaunchOutcome::ReconnectFailed {
                        branch_name: agent_label,
                        message,
                    },
                );
            }
            AgentLaunchFailedOutcome::ForceReconnect {
                session_id,
                agent_label,
                message,
            } => {
                self.resolve_reconnect_op_or(
                    &session_id,
                    dux_core::engine::LaunchOutcome::ForceReconnectFailed {
                        branch_name: agent_label,
                        message,
                    },
                );
            }
            AgentLaunchFailedOutcome::ResumeFallback => {
                // Engine logged + marked Detached; nothing for the view.
            }
            AgentLaunchFailedOutcome::StartupAutoReopen {
                agent_label,
                message,
                ..
            } => {
                self.set_warning(format!(
                    "Couldn't auto-reopen agent \"{agent_label}\": {message}"
                ));
            }
            AgentLaunchFailedOutcome::Tab {
                agent_label,
                message,
                ..
            } => {
                // A tab launch failed (fresh create or dormant relaunch): surface
                // the real error so the user knows why nothing came up. The Engine
                // has already removed a failed fresh-create's row.
                self.set_warning(format!(
                    "Tab launch failed for \"{agent_label}\": {message}"
                ));
            }
            AgentLaunchFailedOutcome::Silent => {
                // Ghost-tab launch failure: the row was already closed by the
                // user, so there is nothing to warn about.
            }
        }
    }

    /// Resolve a stashed reconnect/fresh-restart [`HandlerStatusOp`] (keyed by
    /// session id) against `outcome`, replacing exactly its keyed busy. When no op
    /// is stashed (a launch ready/failed not driven through the reconnect dispatch
    /// sites), fall back to applying the SAME final anonymously via the shared
    /// [`dux_core::engine::launch_outcome_final`] mapping, so the wording is byte-identical to the
    /// pre-op behavior.
    fn resolve_reconnect_op_or(
        &mut self,
        session_id: &str,
        outcome: dux_core::engine::LaunchOutcome,
    ) {
        if let Some(op) = self.pending_reconnect_ops.remove(session_id) {
            self.apply_reaction(op.resolve(&outcome).into_reaction());
            return;
        }
        // No op stashed: apply the SAME final anonymously (no key), preserving the
        // pre-op behavior. `reconnect_final` is the single wording source.
        match dux_core::engine::launch_outcome_final(&outcome) {
            dux_core::engine::Final::Message { tone, text, .. } => {
                self.status.set(std::time::Instant::now(), None, tone, text);
            }
            dux_core::engine::Final::Clear => {
                if matches!(self.status.most_recent_tui(), Some((StatusTone::Busy, _))) {
                    self.set_info(String::new());
                }
            }
        }
    }
}

fn project_persistence_failure_message(action: ProjectPersistenceAction, error: &str) -> String {
    match action {
        ProjectPersistenceAction::Add { project, .. } => format!(
            "Could not save project \"{}\" to the database: {error}",
            project.name,
        ),
        ProjectPersistenceAction::Remove { project_name, .. } => {
            format!("Could not remove project \"{project_name}\" from the database: {error}")
        }
        ProjectPersistenceAction::Delete { project_name, .. } => format!(
            "Could not finish deleting project \"{project_name}\" from the database: {error}"
        ),
        ProjectPersistenceAction::UpdateDefaultProvider { project_name, .. } => {
            format!("Could not save the provider change for project \"{project_name}\": {error}")
        }
        ProjectPersistenceAction::UpdateAutoReopen { project_name, .. } => {
            format!("Could not save the auto-reopen change for project \"{project_name}\": {error}")
        }
        ProjectPersistenceAction::UpdateStartupCommand { project_name, .. } => {
            format!("Could not save the startup command for project \"{project_name}\": {error}")
        }
        ProjectPersistenceAction::UpdateEnv { project_name, .. } => {
            format!("Could not save environment variables for project \"{project_name}\": {error}")
        }
    }
}

fn session_owner_id(pty: &PrunedPty) -> Option<&str> {
    match pty.owner.as_ref().map(TerminalOwner::as_ref) {
        Some(TerminalOwnerRef::Session(session_id)) => Some(session_id),
        Some(TerminalOwnerRef::Project(_) | TerminalOwnerRef::Standalone) | None => None,
    }
}

fn is_session_slot_prune(pty: &PrunedPty, session_id: &str) -> bool {
    pty.kind == PrunedPtyKind::Agent
        && pty.id == session_id
        && session_owner_id(pty) == Some(session_id)
}

fn project_display_name(path: &str, name: &str) -> String {
    if name.trim().is_empty() {
        Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("project")
            .to_string()
    } else {
        name.trim().to_string()
    }
}

fn initial_commit_project_status(
    display_name: &str,
    initialized_repo: bool,
    seeded_gitignore: bool,
) -> String {
    if initialized_repo && seeded_gitignore {
        format!(
            "Initialized a git repository, seeded a starter .gitignore, created an initial commit, and added project \"{display_name}\" to workspace."
        )
    } else if initialized_repo {
        format!(
            "Initialized a git repository, created an initial commit, and added project \"{display_name}\" to workspace."
        )
    } else {
        format!("Created an initial commit and added project \"{display_name}\" to workspace.")
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

/// What a reloaded startup-bound `[server]` change means for this terminal UI.
/// A background listener is restarted from inside dux, so the serving copy names
/// that pair of commands; with nothing serving there is nothing to restart and
/// the change simply waits for the next listener.
pub(crate) fn server_restart_warning(serving_in_background: bool) -> &'static str {
    match serving_in_background {
        true => {
            "Server settings changed in config, but a listener that is already bound cannot adopt \
             them. Stop the background server and start it again to apply them."
        }
        false => {
            "Server settings changed in config. Nothing is serving right now, so they apply the \
             next time a server starts."
        }
    }
}

pub(crate) fn run_create_agent_branch_inspection_job(
    project: Project,
    worker_tx: Sender<WorkerEvent>,
    status_op_id: Option<String>,
) {
    let repo_path = PathBuf::from(&project.path);
    let result = git::current_branch_opt(&repo_path)
        .map_err(|err| {
            format!(
                "Couldn't inspect the current branch for project \"{}\": {err:#}",
                project.name
            )
        })
        .and_then(|maybe_branch| {
            // On a detached HEAD, `maybe_branch` is None; pass None so
            // `leading_branch_for_project` falls back to the remote default or "main".
            let cur = maybe_branch.as_deref();
            let leading_branch = project
                .leading_branch
                .clone()
                .unwrap_or_else(|| leading_branch_for_project(&repo_path, cur));
            if !git::local_branch_exists(&repo_path, &leading_branch) {
                return Err(format!(
                    "Cannot create agent for \"{}\": leading branch \"{}\" no longer exists locally. Restore that branch or re-add the project.",
                    project.name, leading_branch
                ));
            }
            Ok(CreateAgentBranchInspection {
                current_branch: maybe_branch.unwrap_or_default(),
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
            provider: ProviderKind::from_str("custom"),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: "project-1".to_string(),
                    project_path: Some(worktree.to_string_lossy().to_string()),
                    source_branch: "main".to_string(),
                    branch_name: "agent-branch".to_string(),
                    initial_branch: "agent-branch".to_string(),
                    branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                    worktree_path: worktree.to_string_lossy().to_string(),
                },
            ),
        }
    }

    #[test]
    fn persistence_failure_without_pending_status_uses_action_specific_message() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());

        app.apply_project_persistence_outcome(ProjectPersistenceOutcome {
            action: ProjectPersistenceAction::UpdateEnv {
                project_id: "project-1".to_string(),
                project_name: "demo".to_string(),
                env: std::collections::BTreeMap::new(),
            },
            view: ProjectPersistenceView::PersistenceFailed {
                error: "database unavailable".to_string(),
            },
            status_op_id: None,
        });

        assert_eq!(
            app.status.message(),
            "Could not save environment variables for project \"demo\": database unavailable"
        );
    }

    /// Shared scaffolding for the focused-extra-tab exit tests: an extra tab
    /// of the selected session whose CLI exits with `code`, with the user
    /// interactive + fullscreen ON that tab, ticked through `drain_events`.
    fn drain_focused_extra_tab_exit(code: &str) -> crate::app::App {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session_id = app
            .selected_session()
            .expect("test_app selects a session")
            .id
            .clone();
        app.engine.agent_tabs.insert(
            "tab-x".to_string(),
            crate::model::AgentTab {
                id: "tab-x".to_string(),
                session_id: session_id.clone(),
                provider: ProviderKind::from_str("claude"),
                sort_order: 1,
                created_at: Utc::now(),
            },
        );
        let client = crate::pty::PtyClient::spawn(
            "sh",
            &["-c".to_string(), format!("echo hi; exit {code}")],
            Path::new("."),
            10,
            40,
            100,
        )
        .expect("spawn pty");
        app.engine.providers.insert("tab-x".to_string(), client);
        app.focused_tabs
            .insert(session_id.clone(), "tab-x".to_string());
        app.focus = FocusPane::Center;
        app.center_mode = CenterMode::Agent;
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        app.terminal_selection = Some(TerminalSelection {
            anchor: TermGridPos { row: 0, col: 0 },
            end: TermGridPos { row: 0, col: 1 },
            dragging: true,
            origin: app.snapshot_selection_origin(),
        });
        app.raw_input_parser
            .feed_sequences(crate::raw_input::BRACKET_PASTE_START);
        app.in_bracket_paste = true;
        app.raw_input_buf = b"pending".to_vec();
        app.loading_input_buf = b"loading".to_vec();

        // Wait for END OF INPUT *and* a reaped exit status, then let a single
        // drain_events observe it. See `wait_for_pty_eof`: a PTY missing either
        // fact is deliberately held back by REAPED_DRAIN_GRACE, so breaking out
        // on one of them alone makes this assertion flake. The status arm
        // matters most here: without it the clean exit below is pruned as
        // `exit_success: None` and the tab row survives.
        crate::app::test_support::wait_for_pty_eof(&mut app, "tab-x");
        app.drain_events();
        assert!(
            !app.engine.providers.contains_key("tab-x"),
            "the exited tab should have been pruned"
        );
        app
    }

    fn assert_agent_input_cleared(app: &crate::app::App) {
        assert_eq!(app.input_target, InputTarget::None);
        assert!(app.terminal_selection.is_none());
        assert!(!app.in_bracket_paste);
        assert!(app.raw_input_buf.is_empty());
        assert!(app.raw_input_parser.pending().is_empty());
        assert!(!app.raw_input_parser.in_bracket_paste());
        assert!(app.loading_input_buf.is_empty());
    }

    /// #1 regression: the TUI exit-prune teardown must clear EVERY runtime map
    /// keyed by the exited tab via the single-source `clear_tab_runtime`, not a
    /// hand-enumerated subset. The old loop dropped providers/pins/activity/
    /// input but LEAKED `needs_attention`, `pty_progress`, and `agent_viewed`;
    /// on a long-running session that is one stranded entry per exited tab, and
    /// a stale attention/progress flag could resurface on a recycled id.
    #[test]
    fn exit_prune_clears_the_attention_progress_and_viewed_maps() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session_id = app
            .selected_session()
            .expect("test_app selects a session")
            .id
            .clone();
        // A clean-exiting session-slot provider (keyed by the session id).
        let client = crate::pty::PtyClient::spawn(
            "sh",
            &["-c".to_string(), "exit 0".to_string()],
            Path::new("."),
            10,
            40,
            100,
        )
        .expect("spawn pty");
        app.engine.providers.insert(session_id.clone(), client);
        // Seed the three runtime maps the teardown must clear.
        app.engine.needs_attention.insert(session_id.clone());
        app.engine.pty_progress.insert(
            session_id.clone(),
            dux_core::pty::ProgressReport {
                working: true,
                at: std::time::Instant::now(),
            },
        );
        app.engine
            .agent_viewed
            .insert(session_id.clone(), std::time::Instant::now());

        // END OF INPUT *and* a reaped status: with either one missing the prune
        // is deliberately deferred inside REAPED_DRAIN_GRACE and one drain would
        // see nothing. See `wait_for_pty_eof`.
        crate::app::test_support::wait_for_pty_eof(&mut app, &session_id);
        app.drain_events();

        assert!(!app.engine.providers.contains_key(&session_id), "pruned");
        assert!(
            !app.engine.needs_attention.contains(&session_id),
            "needs_attention must be cleared on exit prune"
        );
        assert!(
            !app.engine.pty_progress.contains_key(&session_id),
            "pty_progress must be cleared on exit prune"
        );
        assert!(
            !app.engine.agent_viewed.contains_key(&session_id),
            "agent_viewed must be cleared on exit prune"
        );
    }

    /// A CLEAN exit (code 0) of the focused extra tab closes the tab itself:
    /// the user deliberately ended that conversation (e.g. /exit), so the row
    /// is deleted and — with no live sibling left — the pane minimizes and
    /// focus lands in the list, exactly like a single agent's clean exit.
    #[test]
    fn focused_extra_tab_clean_exit_closes_the_tab_and_minimizes() {
        let app = drain_focused_extra_tab_exit("0");
        assert!(
            !app.engine.agent_tabs.contains_key("tab-x"),
            "a clean exit must close the tab (delete its row)"
        );
        assert_agent_input_cleared(&app);
        assert_eq!(
            app.fullscreen_overlay,
            FullscreenOverlay::None,
            "the pane minimizes like a single agent's clean exit"
        );
        assert_eq!(
            app.focus,
            FocusPane::Left,
            "with no live sibling the user lands back in the list"
        );
    }

    /// A CRASH (non-zero exit) of the focused extra tab keeps the tab: the
    /// dormant relaunch screen is the crash-diagnosis surface, so the row
    /// survives and the fullscreen overlay stays up — but interactive input
    /// still drops immediately so every escape hatch works.
    #[test]
    fn focused_extra_tab_crash_keeps_the_dormant_tab() {
        let app = drain_focused_extra_tab_exit("3");
        assert!(
            app.engine.agent_tabs.contains_key("tab-x"),
            "a crash must keep the tab row for diagnosis/relaunch"
        );
        assert_agent_input_cleared(&app);
        assert_eq!(
            app.fullscreen_overlay,
            FullscreenOverlay::Agent,
            "the fullscreen dormant-tab (relaunch) screen stays up"
        );
    }

    #[test]
    fn create_failure_disarms_the_tui_origin_before_the_event_is_processed() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        app.create_agent_started_here = true;
        app.engine
            .worker_tx
            .send(WorkerEvent::CreateAgentFailed {
                status_op_id: "missing-op".to_string(),
                message: "creation failed".to_string(),
            })
            .expect("send create failure");

        app.drain_events();

        assert!(!app.create_agent_started_here);
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

    #[test]
    fn nested_multi_reactions_apply_status_leaves_in_order() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());

        app.apply_reaction(EventReaction::Multi(vec![
            EventReaction::Status(StatusUpdate::info("first")),
            EventReaction::Multi(vec![EventReaction::Status(StatusUpdate::warning("last"))]),
        ]));

        let (tone, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(tone, StatusTone::Warning);
        assert_eq!(message, "last");
    }

    #[test]
    fn browser_entries_update_only_the_matching_open_directory() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let root = PathBuf::from(&app.engine.projects[0].path);
        let entry = BrowserEntry {
            path: root.join("child"),
            label: "child/".to_string(),
            is_git_repo: false,
            is_parent: false,
        };
        app.prompt = PromptState::BrowseProjects {
            purpose: BrowsePurpose::AddProject,
            current_dir: root.clone(),
            entries: Vec::new(),
            loading: true,
            selected: 7,
            filter: TextInput::new(),
            searching: false,
            editing_path: false,
            path_input: TextInput::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        };

        app.apply_reaction(EventReaction::BrowserEntriesArrived {
            dir: root.join("stale"),
            entries: vec![entry.clone()],
        });
        match &app.prompt {
            PromptState::BrowseProjects {
                entries,
                loading,
                selected,
                ..
            } => {
                assert!(entries.is_empty());
                assert!(*loading);
                assert_eq!(*selected, 7);
            }
            other => panic!("expected browser prompt, got {other:?}"),
        }

        app.apply_reaction(EventReaction::BrowserEntriesArrived {
            dir: root,
            entries: vec![entry],
        });
        match &app.prompt {
            PromptState::BrowseProjects {
                entries,
                loading,
                selected,
                ..
            } => {
                assert_eq!(entries.len(), 1);
                assert!(!loading);
                assert_eq!(*selected, 0);
            }
            other => panic!("expected browser prompt, got {other:?}"),
        }
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
            tab_id: session.id.clone(),
            session,
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
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

    /// Creating an agent moves the cursor onto the new agent, so a lingering
    /// `manage-projects` target must be cleared — otherwise a follow-up project
    /// action would resolve the stale pick instead of the new agent's project.
    #[test]
    fn create_committed_view_clears_manage_projects_target() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        // A prior manage-projects pick targeted some other project.
        app.project_chooser_context = Some("some-other-project".to_string());

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.id.clone(),
            session,
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::CreateCommitted {
                status_message: "Created agent.".to_string(),
                startup_result_error: None,
            },
        });

        assert!(
            app.project_chooser_context.is_none(),
            "creating an agent must clear the manage-projects target",
        );
    }

    /// A completed launch lands focused-but-minimized. The
    /// Reconnect ready with `wants_fullscreen: false` must put focus on the
    /// Center pane with NO fullscreen overlay and NO interactive input
    /// target, leaving the pane typeable (the derived predicate).
    #[test]
    fn reconnect_ready_lands_focused_but_minimized_and_typeable() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();
        // A live provider under the session-slot tab id, as the completed
        // launch would have inserted engine-side.
        let client = crate::pty::PtyClient::spawn(
            "sh",
            &["-c".to_string(), "sleep 0.5".to_string()],
            Path::new("."),
            10,
            40,
            100,
        )
        .expect("spawn pty");
        app.engine.providers.insert(session.id.clone(), client);
        app.focus = FocusPane::Left;

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.id.clone(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".to_string(),
            },
        });

        assert_eq!(app.focus, FocusPane::Center, "the launch focuses Center");
        assert_eq!(
            app.input_target,
            InputTarget::None,
            "a minimized landing must not enter interactive mode"
        );
        assert_eq!(
            app.fullscreen_overlay,
            FullscreenOverlay::None,
            "a minimized landing must not fullscreen"
        );
        assert!(
            app.center_typeable(),
            "the landed pane must be immediately typeable"
        );
    }

    /// The one exception to minimized landings: a fullscreen-seeking launch (the request's
    /// `wants_fullscreen` bit) still lands fullscreen-interactive.
    #[test]
    fn fullscreen_seeking_reconnect_ready_lands_fullscreen() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.id.clone(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: true,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".to_string(),
            },
        });

        assert_eq!(app.input_target, InputTarget::Agent);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Agent);
    }

    /// A create is never fullscreen-seeking: the CreateCommitted ready lands
    /// the fresh agent focused-but-minimized.
    #[test]
    fn create_committed_ready_lands_minimized() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.id.clone(),
            session,
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::CreateCommitted {
                status_message: "Created agent.".to_string(),
                startup_result_error: None,
            },
        });

        assert_eq!(app.focus, FocusPane::Center);
        assert_eq!(app.input_target, InputTarget::None);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
    }

    /// The engine-initiated resume-fallback relaunch is never
    /// fullscreen-seeking; when its ready arrives for the selected session it
    /// lands minimized too.
    #[test]
    fn resume_fallback_ready_lands_minimized_for_the_selected_session() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.id.clone(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::ResumeFallback {
                session_id: session.id.clone(),
                status_message: "Fresh restart.".to_string(),
            },
        });

        assert_eq!(app.input_target, InputTarget::None);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
    }

    /// Reconnecting a dormant agent flips it Active in the engine; the view must
    /// rebuild the flat list so the agent leaves the collapsed Inactive tail and
    /// rejoins the active section (regression: the Reconnect arm forgot to
    /// rebuild, stranding a just-reactivated agent under Inactive).
    #[test]
    fn reconnect_moves_a_reactivated_agent_out_of_the_inactive_tail() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();
        assert!(
            matches!(
                app.engine.sessions[0].status,
                crate::model::SessionStatus::Detached
            ),
            "fixture precondition: the seeded agent starts Detached",
        );
        assert!(
            app.left_items()
                .iter()
                .any(|i| matches!(i, LeftItem::InactiveToggle)),
            "a Detached agent sits under an Inactive toggle before reconnect",
        );

        // The engine marks the session Active while (re)launching it; mirror that,
        // then apply the reconnect view without rebuilding the list by hand.
        app.engine
            .mark_session_status(&session.id, crate::model::SessionStatus::Active);
        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.id.clone(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".to_string(),
            },
        });

        assert!(
            !app.left_items()
                .iter()
                .any(|i| matches!(i, LeftItem::InactiveToggle)),
            "after reconnect no dormant agents remain, so the Inactive tail is gone",
        );
        assert!(
            matches!(app.left_items().first(), Some(LeftItem::Session(0))),
            "the reactivated agent must render in the active section",
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
        let op = app.build_reconnect_status_op(format!(
            "Launching agent \"{}\"...",
            session.branch_name().expect("managed test session")
        ));
        let op_key = op.id().to_string();
        app.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
        app.pending_reconnect_ops.insert(session.id.clone(), op);

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.id.clone(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
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
        // The engine's message survives verbatim at the front; the TUI appends
        // its landing note (typeable pane, named fullscreen key) because the
        // minimized landing is a TUI-only concept the shared message can't know.
        assert!(
            entry.message.starts_with("Reconnected."),
            "the engine-composed message must lead: {:?}",
            entry.message
        );
        let key = app.bindings.label_for(Action::ToggleFullscreen);
        assert!(
            entry.message.contains("type to the agent")
                && entry
                    .message
                    .contains(&format!("press {key} for fullscreen")),
            "a minimized landing must say the pane is typeable and name the \
             fullscreen toggle via the bindings: {:?}",
            entry.message
        );
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

        let op = app.build_reconnect_status_op(format!(
            "Launching agent \"{}\"...",
            session.branch_name().expect("managed test session")
        ));
        let op_key = op.id().to_string();
        app.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
        app.pending_reconnect_ops.insert(session.id.clone(), op);

        app.apply_agent_launch_failed_view(AgentLaunchFailedOutcome::Reconnect {
            session_id: session.id.clone(),
            agent_label: "feat".to_string(),
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
            agent_label: "feat".to_string(),
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
        let session = test_session(tmp.path());
        let request = AgentLaunchRequest {
            tab_id: session.id.clone(),
            provider: session.provider.clone(),
            session,
            provider_config: crate::config::ProviderCommandConfig {
                command: "definitely-missing-provider-command".to_string(),
                args: vec!["--ignored".to_string()],
                ..Default::default()
            },
            resume: false,
            pty_size: (24, 80),
            scrollback_lines: 1_000,
            env: Vec::new(),
            identity: Default::default(),
            kind: AgentLaunchKind::Reconnect {
                status_message: "reconnect".to_string(),
            },
            wants_fullscreen: false,
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
            provider: ProviderKind::from_str("codex"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: project.id.clone(),
                    project_path: Some(project.path.clone()),
                    source_branch: "main".to_string(),
                    branch_name: "agent-branch".to_string(),
                    initial_branch: "agent-branch".to_string(),
                    branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                    worktree_path: tmp.path().join("source").to_string_lossy().to_string(),
                },
            ),
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
            dux_core::term_identity::TerminalIdentity::default(),
        );

        match worker_rx.recv().expect("worker event") {
            WorkerEvent::CreateAgentFailed { message, .. } => {
                assert_eq!(message, "Forking an agent requires choosing a name first.");
            }
            _ => panic!("expected missing-name failure"),
        }
        assert!(worker_rx.try_recv().is_err());
    }

    /// A `[server]` setting only takes effect when a listener binds, so the
    /// terminal UI must say so on its own reload rather than leaving the news to
    /// a browser that may not be connected.
    #[test]
    fn a_reload_that_changes_a_server_setting_warns_on_the_terminal_ui() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let mut config = app.engine.config.clone();
        config.server.port += 1;

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

        let (tone, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(tone, StatusTone::Warning, "the last word is the warning");
        assert!(
            message.contains("server"),
            "the warning names what needs restarting: {message}"
        );
    }

    #[test]
    fn a_reload_that_leaves_the_server_section_alone_warns_about_nothing() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let mut config = app.engine.config.clone();
        config.ui.diff_tab_width += 1;

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

        let (tone, _) = app.status.most_recent_tui().expect("a status");
        assert_eq!(tone, StatusTone::Info, "nothing bound has drifted");
    }

    /// `color` reaches only the `dux server` console, which neither the flip nor
    /// the background server builds, so telling a terminal UI user to restart
    /// anything would name a restart that changes nothing they can see.
    #[test]
    fn a_reload_that_changes_only_the_console_color_warns_about_nothing() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let mut config = app.engine.config.clone();
        config.server.color = "never".to_string();

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

        let (tone, _) = app.status.most_recent_tui().expect("a status");
        assert_eq!(
            tone,
            StatusTone::Info,
            "nothing the terminal UI binds moved"
        );
    }

    /// The copy is chosen by whether a listener is up on this process, so the
    /// choice must read the live companion rather than a remembered flag.
    #[test]
    fn a_serving_terminal_ui_gets_the_stop_and_start_wording_on_a_bind_change() {
        let (companion, _recorded) = crate::app::background_server::tests::FakeCompanion::serving();
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        app.engine.config.server.serve_while_tui = true;
        app.companion = Some(companion);
        let mut config = app.engine.config.clone();
        // Kept on, or the reload's own live switch stops the serve before the
        // warning is chosen and the copy would be right for the wrong reason.
        config.server.serve_while_tui = true;
        config.server.port += 1;

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

        let (tone, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(tone, StatusTone::Warning);
        assert_eq!(
            message,
            server_restart_warning(true),
            "a serving companion picks the stop-and-start copy"
        );
    }

    #[test]
    fn an_idle_terminal_ui_gets_the_next_start_wording_on_a_bind_change() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        assert!(!app.background_server_is_serving());
        let mut config = app.engine.config.clone();
        config.server.port += 1;

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

        let (_, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(message, server_restart_warning(false));
    }

    /// The restart is owed until the user performs it, so unlike an ordinary
    /// warning this one is not on a timer.
    #[test]
    fn the_server_restart_warning_holds_the_line_until_the_user_acts() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let window = std::time::Duration::from_secs(6);
        app.status.set_clear_after(window);
        let mut config = app.engine.config.clone();
        config.server.port += 1;

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

        let now = std::time::Instant::now();
        let _ = app
            .status
            .tick(now + window * 4, dux_core::statusline::BUSY_TIMEOUT);
        let (_, message) = app
            .status
            .most_recent_tui()
            .expect("the restart warning must survive the warning window");
        assert_eq!(message, server_restart_warning(false));
    }

    #[test]
    fn the_server_restart_warning_names_the_background_server_only_while_it_serves() {
        let serving = server_restart_warning(true);
        let idle = server_restart_warning(false);
        assert_ne!(serving, idle);
        assert!(
            serving.contains("background server"),
            "a serving terminal UI is told what to stop and start: {serving}"
        );
        assert!(
            !idle.contains("background server"),
            "an idle one is told when the change applies instead: {idle}"
        );
    }

    /// The pull is best-effort: a broken checkout does not abort creation at the
    /// pull stage; the job proceeds and fails later, on the real problem (here:
    /// the leading branch cannot exist in a directory that is not a repo).
    #[test]
    fn fresh_worker_survives_pull_failure_and_fails_on_the_missing_repo_instead() {
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
                copy_uncommitted_changes: false,
            },
            paths,
            Config::default(),
            worker_tx,
            (80, 24),
            "op-create-1".to_string(),
            dux_core::term_identity::TerminalIdentity::default(),
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
        let mut failure = None;
        while let Ok(event) = worker_rx.try_recv() {
            if let WorkerEvent::CreateAgentFailed { message, .. } = event {
                failure = Some(message);
            }
        }
        let failure = failure.expect("a directory that is not a repo must still fail creation");
        assert!(
            !failure.contains("Failed to pull latest changes"),
            "the pull must not abort creation: {failure}"
        );
        assert!(
            failure.contains("leading branch \"main\" no longer exists locally"),
            "creation fails on the real problem instead: {failure}"
        );
    }
}
