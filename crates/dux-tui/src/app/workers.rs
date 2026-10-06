use std::sync::mpsc::Sender;

use dux_core::config_reload_status::ConfigReloadOutcome;
use dux_core::engine::{
    AgentLaunchFailedOutcome, AgentLaunchReadyOutcome, AgentLaunchReadyView,
    BeginDeleteSessionOutcome, BeginDeleteSessionView, DeleteTerminalView, DispatchAgentLaunchView,
    EventReaction, FinishDeleteSessionView, ProjectPersistenceOutcome, ProjectPersistenceView,
    PrunedPty, PrunedPtyKind, StatusUpdate, WorktreeRemoval, closed_tab_exit_notice,
};

use super::*;

impl PruneViewContext {
    fn capture(app: &App) -> Self {
        let selected_session = app.selected_session().map(|session| session.id.clone());
        let focused_tab = selected_session
            .as_ref()
            .map(|session_id| app.focused_tab_id(session_id));
        // Extra tabs only: a closed tab's sentence comes from the prune itself,
        // and the slot tab's own exit is the agent's notice rather than a tab's.
        let tab_providers: HashMap<String, String> = app
            .engine
            .agent_tabs
            .iter()
            .map(|(id, tab)| (id.as_str().to_string(), tab.provider.as_str().to_string()))
            .collect();
        let selected_slot_tab = selected_session.as_ref().and_then(|session_id| {
            app.engine
                .sessions
                .iter()
                .find(|session| &session.id == session_id)
                .map(|session| session.slot_tab_id().as_str().to_string())
        });
        Self {
            selected_session,
            focused_tab,
            selected_slot_tab,
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
        self.drain_unpushed_count();
        self.drain_branch_listing();
        self.drain_pending_diff();
        self.drain_changes_tree_work();
        self.drain_worker_events();
        self.apply_resume_fallback_sweep();
        self.apply_reaped_terminations();
        let maintenance = self.apply_pruned_pty_events();
        self.note_companion_maintenance(&maintenance);
        self.refresh_resource_monitor_if_due();
        self.engine.sync_has_active_processes();
    }

    fn apply_pruned_pty_events(&mut self) -> dux_core::background_serve::DrainedMaintenance {
        let context = PruneViewContext::capture(self);
        let pruned = self.engine.prune_exited_ptys();
        if !pruned.is_empty() {
            self.mark_frame_dirty();
        }
        let reported_prunes = if self.background_server_is_serving() {
            pruned.clone()
        } else {
            Vec::new()
        };
        self.apply_pruned_agent_tabs(&pruned, &context);
        let reported = self.apply_selected_agent_exit(&pruned, &context);
        self.apply_unselected_agent_exits(&pruned, reported.as_deref());
        self.apply_pruned_terminals(&pruned);
        let foregrounds_changed = self.engine.refresh_terminal_foregrounds();
        if foregrounds_changed {
            // A terminal row's label is its foreground command.
            self.mark_frame_dirty();
        }
        dux_core::background_serve::DrainedMaintenance {
            pruned: reported_prunes,
            foregrounds_changed,
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
        let support_provider = (!self
            .engine
            .is_slot_tab_of(SessionIdRef::new(session_id), TabIdRef::new(&pty.id)))
        .then(|| {
            context
                .tab_providers
                .get(&pty.id)
                .cloned()
                .unwrap_or_default()
        });
        if pty.tab_closed {
            self.apply_closed_agent_tab(pty, session_id, was_focused_tab);
            true
        } else {
            self.apply_exited_agent_tab(support_provider, was_focused_tab);
            false
        }
    }

    fn apply_closed_agent_tab(&mut self, pty: &PrunedPty, session_id: &str, was_focused_tab: bool) {
        // The prune's own account of the close, in the sentence the browser
        // gets: the row is gone, so nothing here could look these facts up.
        if let Some(closed) = &pty.closed_tab {
            self.set_info(closed_tab_exit_notice(closed));
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

    /// Report the selected agent's own exit, and answer with the pty it spoke
    /// for so the workspace-wide sweep does not say it twice.
    fn apply_selected_agent_exit(
        &mut self,
        pruned: &[PrunedPty],
        context: &PruneViewContext,
    ) -> Option<String> {
        let current_id = self.selected_session().map(|session| session.id.clone())?;
        let slot_before_prune = context.selected_slot_tab.as_deref();
        let pty = pruned
            .iter()
            .find(|pty| is_agent_exit_prune(&self.engine, pty, &current_id, slot_before_prune))?;
        let focused = self.focused_tab_id(&current_id);
        if !self
            .engine
            .is_slot_tab_of(SessionIdRef::new(&current_id), TabIdRef::new(&focused))
            && self.engine.providers.contains_key(TabIdRef::new(&focused))
        {
            return None;
        }
        self.apply_selected_agent_exit_status(pty);
        Some(pty.id.clone())
    }

    /// Say that an agent's last live tab exited even when the user is looking at
    /// a different agent.
    ///
    /// An agent detaching is a fact about the workspace, not about the pane in
    /// front of you, and this surface used to make it conditional on selection:
    /// a browser was told every time and the terminal UI only when the agent
    /// happened to be the selected one, so the same exit produced two different
    /// screens. The sentence is the shared one; the selected agent keeps the
    /// richer line above, which names the pane's own way back.
    fn apply_unselected_agent_exits(&mut self, pruned: &[PrunedPty], reported: Option<&str>) {
        let key = self.bindings.label_for(Action::ReconnectAgent);
        let remedy = format!(
            "Select the agent and press \"{key}\" to relaunch it, or run \
             force-reconnect-agent from the palette to start a fresh session."
        );
        let notices: Vec<String> = pruned
            .iter()
            .filter(|pty| {
                pty.kind == PrunedPtyKind::Agent
                    && pty.agent_detached
                    && reported != Some(pty.id.as_str())
            })
            .map(|pty| dux_core::engine::detached_agent_notice(pty, &remedy).into())
            .collect();
        for notice in notices {
            self.set_warning(notice);
        }
    }

    fn apply_selected_agent_exit_status(&mut self, pty: &PrunedPty) {
        let key = self.bindings.label_for(Action::ReconnectAgent);
        if self.session_surface != SessionSurface::Agent {
            self.set_info(dux_core::engine::agent_exit_with_companion_notice(
                pty,
                &format!("Press \"{key}\" to relaunch the agent."),
            ));
            return;
        }
        self.log_minimal_agent_exit(pty);
        let status = pruned_agent_exit_message(pty, &key);
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.focus = FocusPane::Left;
        // A read error is dux killing the agent, not the agent ending, so it
        // reads as an error even though it left no exit status behind to say so.
        if pty.exit_success == Some(false) || pty.read_error.is_some() {
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
            // A drained event is a redraw source whatever it turns out to be.
            self.mark_frame_dirty();
            let metadata = DrainedEventMetadata::capture(&event);
            self.disarm_tui_launch_for_failed_event(&event);
            let reaction = self.engine.process_worker_event(event);
            self.note_changed_files_owner(&metadata);
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

    /// A successful changed-files read for the worktree still watched is what
    /// the engine just applied, so the lists now belong to the watched agent.
    /// Recorded before the reaction runs, because that reaction is what
    /// reconciles the expanded folders against those lists.
    fn note_changed_files_owner(&mut self, metadata: &DrainedEventMetadata) {
        let Some(answer) = &metadata.changed_files_answer else {
            return;
        };
        if answer.error.is_some() {
            return;
        }
        let watched = self
            .engine
            .watched_worktree
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        if watched.as_deref() == Some(answer.worktree.as_path()) {
            self.changes_tree.lists_for = self.engine.watched_session_id.clone();
        }
    }

    fn apply_resume_fallback_sweep(&mut self) {
        let sweep_size = self.pty_size_for_launch();
        for reaction in self.engine.sweep_resume_fallbacks(sweep_size) {
            self.mark_frame_dirty();
            let routing = self.companion_routing();
            self.notify_companion(&reaction);
            self.apply_routed_reaction(reaction, &routing);
        }
    }

    /// Apply one reaper pass: dispatch the worktree removals that were waiting
    /// on a dead process, and deliver the outcomes of completed detach requests
    /// to the status line.
    pub(crate) fn apply_reaped_terminations(&mut self) {
        let reaped = self.engine.reap_terminating_ptys();
        for removal in reaped.removals {
            self.mark_frame_dirty();
            let _busy = self.engine.dispatch_deferred_worktree_removal(removal);
        }
        // A detach request's spinner is retired by what actually happened to the
        // child, not by a timer: the reaper is the only place that knows whether
        // the agent went on its own or had to be forced.
        //
        // A detach a BROWSER started lands on this line too, by design: while
        // this process is serving, one agent stopping is a fact about the shared
        // workspace, not about the tab that asked for it.
        for outcome in reaped.detach_finals {
            self.mark_frame_dirty();
            let routing = self.companion_routing();
            let reaction = outcome.into_reaction();
            self.notify_companion(&reaction);
            self.apply_routed_reaction(reaction, &routing);
        }
    }

    fn disarm_tui_launch_for_failed_event(&mut self, event: &WorkerEvent) {
        match event {
            WorkerEvent::AgentLaunchFailed(data) => {
                self.tui_launched_ptys.remove(data.request.tab_id.as_str());
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
        // Origin routing: while the background web server is on, both surfaces
        // see the same worker events, and a reaction's follow-up (a git job, an
        // added project, an agent create) belongs to whichever surface asked for
        // it, which the engine is the one to know (see
        // `dux_core::engine::owner_of_reaction`). Checked here rather than per
        // arm so a `Multi` routes leaf by leaf, carrying the same verdict source
        // down: a leaf must not be judged against a map the fanout has emptied.
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
            // The terminal UI reads `Engine::gh_status` live every time it gates
            // a pull-request command, so the flip is already visible here and
            // the accompanying keyed status carries the explanation. The arm
            // exists so a surface that DOES need to react cannot forget to.
            EventReaction::GhAvailabilityChanged { .. } => {}
            EventReaction::Status(StatusUpdate {
                tone,
                message,
                key,
                quiet_on,
                ..
            }) => {
                // A final ends whatever a change of this surface's still holds
                // under its key, shown or not.
                if tone != StatusTone::Busy
                    && let Some(key) = &key
                {
                    self.engine.attachments.finish_key(key);
                }
                // Only an INFO may be withheld: a warning, an error and a
                // spinner all report something the screen cannot be standing in
                // for, whatever the site asked for.
                if quiet_on.tui && tone == StatusTone::Info {
                    // A keyed final still has a spinner to take down: the
                    // sentence goes, the busy must not be stranded behind it.
                    // A sticky busy refuses to go, and there the sentence is
                    // shown instead, because a spinner nothing retires is worse
                    // than a line nobody needed.
                    let stranded = match &key {
                        Some(key) => !self.status.clear(key, None),
                        None => false,
                    };
                    if !stranded {
                        return;
                    }
                }
                // A commit's final: a landed commit clears the typed message, a
                // refused one leaves it for another try.
                if tone != StatusTone::Busy
                    && let (Some(key), Some((pending, committed))) = (&key, &self.pending_commit)
                    && key == pending
                {
                    // Only the message that was committed is cleared: one typed
                    // since, for the next commit, stays.
                    if tone == StatusTone::Info && self.commit_input.text == *committed {
                        self.commit_input.clear();
                    }
                    self.pending_commit = None;
                }
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
            EventReaction::ClampFilesCursor => {
                // A fresh changed-files read: expanded folders follow it before
                // the cursor is clamped to the rows that result.
                self.reconcile_changes_tree();
                self.clamp_files_cursor();
            }

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
            EventReaction::WorktreeRemoveWaiting {
                session_id,
                message,
            } => {
                // The delete's own spinner says what the removal is waiting for.
                if let Some(op) = self.pending_delete_ops.get(&session_id) {
                    let progress = op.progress(message);
                    self.apply_reaction(EventReaction::Status(progress));
                }
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

            EventReaction::AddProjectAfterClone(done) => {
                // The engine adds the project and dispatches the agent; this
                // surface shows what it says, and, as for a create dispatched
                // here, the agent it starts is this surface's.
                let create = dux_core::engine::InFlightKey::CreateAgent;
                let was_in_flight = self.engine.is_in_flight(&create);
                let finished = self.engine.finish_clone(&done);
                if !was_in_flight && self.engine.is_in_flight(&create) {
                    self.create_agent_started_here = true;
                }
                self.apply_routed_reaction(finished, routing);
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
            EventReaction::ConfigAdopted {
                before,
                github_was_enabled,
                error,
            } => self.apply_config_adopted(*before, github_was_enabled, error),

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

            EventReaction::BeginDeleteSessionView(view) => {
                self.apply_begin_delete_session_view(*view);
            }

            EventReaction::DispatchAgentLaunchView(view) => {
                self.apply_dispatch_agent_launch_view(*view);
            }

            EventReaction::DeleteTerminalView(view) => {
                self.apply_delete_terminal_view(*view);
            }

            EventReaction::ServerFlipPreflightReady {
                result,
                warning,
                startup,
            } => {
                self.apply_server_flip_preflight(result, warning, startup);
            }

            EventReaction::BackgroundServerPreflightReady {
                result,
                warning,
                startup,
            } => {
                self.apply_background_server_preflight(result, warning, startup);
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

    pub(crate) fn apply_reloaded_config_reaction(&mut self, config: Config) {
        let before = self.engine.config.clone();
        let github_was_enabled = self.engine.github_integration_enabled;
        let fallback = config.clone();
        let mut apply_error = None;
        let outcome = match self.apply_reloaded_config(config) {
            Err(error) => {
                // The view could not take the new config, but the engine still
                // adopts it, so memory, the writer's base and the file agree
                // and nothing old is saved over the new file. Everything a
                // swap owes then runs against it, as after a successful
                // apply, so no setting is claimed that is not in force; the
                // failure is said last, so it holds the line.
                self.engine.keep_reloaded_config(fallback);
                self.run_config_swap_effects(&before, github_was_enabled);
                self.note_config_adopted(&before);
                let error = format!("{error:#}");
                apply_error = Some(error.clone());
                TuiConfigReloadOutcome::ApplyFailed(error)
            }
            Ok(()) => TuiConfigReloadOutcome::Applied,
        };
        if let Some(op) = self.pending_config_reload_op.take() {
            self.apply_reaction(op.resolve(&outcome).into_reaction());
        }
        self.post_config_reload_outcome(&outcome);
        // This surface owns the reload here, so a client that asked for it
        // hears how it ended.
        let told = match apply_error {
            Some(error) => ConfigReloadOutcome::ApplyFailed(error),
            None => ConfigReloadOutcome::Applied {
                notes: self.note_config_adopted(&before).into_iter().collect(),
            },
        };
        self.engine.finish_config_reload_operations(&told);
    }

    /// The engine adopted a reloaded config its own apply could not finish
    /// (see `EventReaction::ConfigAdopted`): the view takes it, everything a
    /// reload's swap owes runs against `before`, the config it replaced, and
    /// the reload answers as an apply that failed, the same answer the
    /// terminal UI's own failed apply gives. `github_was_enabled` is the
    /// engine's own state before the reload; its apply asked `gh` for
    /// nothing, so this surface asks where a reload owes it.
    fn apply_config_adopted(&mut self, before: Config, github_was_enabled: bool, error: String) {
        let adopted = self.engine.config.clone();
        // The failure is the message, so the theme's own warning is left to
        // the next reload.
        let _ = self.take_reload_view_state(&adopted);
        self.run_config_swap_effects(&before, github_was_enabled);
        self.note_config_adopted(&before);
        let outcome = TuiConfigReloadOutcome::ApplyFailed(error.clone());
        if let Some(op) = self.pending_config_reload_op.take() {
            self.apply_reaction(op.resolve(&outcome).into_reaction());
        }
        self.post_config_reload_outcome(&outcome);
        self.engine
            .finish_config_reload_operations(&ConfigReloadOutcome::ApplyFailed(error));
    }

    /// Hand a new config's `[server]` section to the background server; returns the
    /// restart-needed warning it raised, so a client that asked for the reload hears it.
    fn note_config_adopted(&mut self, before: &Config) -> Option<String> {
        if let Some(companion) = self.companion.as_mut() {
            companion.note_config_applied(&self.engine.config);
        }
        // The listener's bind settings and the server log's are both read when a
        // serve starts, so a running one cannot adopt either.
        let mut owed: Vec<String> = Vec::new();
        let mut changed =
            dux_core::config::server_bind_setting_names(&before.server, &self.engine.config.server);
        changed.extend(dux_core::config::server_log_file_setting_names(
            &before.server,
            &self.engine.config.server,
        ));
        if !changed.is_empty() {
            let serving = self.background_server_is_serving();
            owed.push(server_restart_warning(serving, &changed));
        }
        owed.extend(dux_core::control_socket::moved_warning(
            &before.server,
            &self.engine.config.server,
        ));
        if owed.is_empty() {
            return None;
        }
        let warning = owed.join(" ");
        self.set_pinned_warning(warning.clone());
        Some(warning)
    }

    /// Answer the reload on the worker lane, so the browsers that were told to
    /// refetch learn whether the config actually took.
    ///
    /// The web announces the reload pre-consume and cannot know this: its
    /// sentence is true about the read and silent about the apply. The lane is
    /// what carries one keyed final to both surfaces, on the key that premature
    /// sentence was raised under, so a failure replaces it there and reads the
    /// same words here. A validation failure never reached a browser at all, so
    /// it keeps the op's own error line and posts nothing.
    fn post_config_reload_outcome(&mut self, outcome: &TuiConfigReloadOutcome) {
        let status = match outcome {
            TuiConfigReloadOutcome::Applied => dux_core::config_reload_status::applied(),
            TuiConfigReloadOutcome::ApplyFailed(error) => {
                dux_core::config_reload_status::adopted_but_apply_failed(error)
            }
            TuiConfigReloadOutcome::ValidationFailed => return,
        };
        self.engine.post_status(status);
    }

    fn apply_open_config_reload_failed_modal(&mut self, message: String) {
        self.engine
            .finish_config_reload_operations(&ConfigReloadOutcome::Refused(message.clone()));
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
        startup: dux_core::serve_log::StartupNotes,
    ) {
        self.server_flip_preflight_pending = false;
        match result {
            Ok((listeners, urls)) => {
                self.apply_server_flip_preflight_success(listeners, urls, warning, startup)
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
        startup: dux_core::serve_log::StartupNotes,
    ) {
        let url_list = urls.join(", ");
        if let Some(warning) = warning {
            if let Some(op) = self.pending_server_flip_op.take() {
                self.apply_reaction(
                    op.resolve(&TuiServerFlipOutcome::Warned(format!(
                        "{warning} Starting the web server on {url_list}. Your agents keep running."
                    )))
                    .into_reaction(),
                );
            }
        } else if let Some(op) = &self.pending_server_flip_op {
            let progress = op.progress(format!(
                "Starting the web server on {url_list}. Your agents keep running."
            ));
            self.apply_reaction(EventReaction::Status(progress));
        }
        self.pending_server_flip = Some(super::PendingServerFlip {
            listeners,
            urls,
            startup,
        });
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
                // The delete's own busy is dismissed first, so it does not
                // outlive the failure; the failure is the line that stays.
                if let Some(op) = op {
                    self.apply_reaction(
                        op.resolve(&TuiDeleteOutcome::SucceededGone {
                            our_busy_still_showing: false,
                        })
                        .into_reaction(),
                    );
                }
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
                    let op = self.build_delete_status_op(&session_id, busy_message.to_string());
                    let pending = self.engine.begin_status_op(&op);
                    self.apply_reaction(EventReaction::Status(pending));
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
        // Logs asked for from a project's action list land over it, and
        // closing them steps back to it.
        let return_to = match std::mem::replace(&mut self.prompt, PromptState::None) {
            PromptState::ProjectActions(list) => Some(Box::new(list)),
            _ => None,
        };
        self.prompt = PromptState::StartupCommandLogs(StartupCommandLogPrompt {
            return_to,
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
            self.set_info(dux_core::engine::closed_terminal_notice(&label));
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
            // The head branch keeps its exact bytes; the seed is what the
            // prompt shows and the user edits, so it drops the bidi controls
            // that would redraw the dialog around it.
            head_branch: pr.head_ref_name.clone(),
            custom_name: Some(dux_core::bidi::strip_bidi_controls(&pr.head_ref_name)),
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
        let success_message = super::render::project_env_saved_message(env_count, &project_name);
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
                self.apply_added_project(status_message.to_string());
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
        // This arm runs on both surfaces (see `owner_of_reaction`), so a child
        // this surface started has to be recognised rather than assumed: an id
        // armed at dispatch for every ordinary launch, and the create flag for a
        // create, whose session id is minted in the worker and so cannot be armed
        // by id at all. Both are spent whether or not the claim happens, and only
        // a launch that really produced a child is claimed: a claim against an id
        // no pty answers to is a driver nobody can see or take over.
        let armed = self.tui_launched_ptys.remove(&outcome.tab_id);
        let created_here = matches!(
            &outcome.view,
            AgentLaunchReadyView::CreateCommitted { .. }
                | AgentLaunchReadyView::CreatePersistFailed { .. }
        ) && std::mem::take(&mut self.create_agent_started_here);
        if (armed || created_here)
            && self
                .engine
                .providers
                .contains_key(TabIdRef::new(&outcome.tab_id))
        {
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
                // The agent list scrolls to its cursor only while the cursor is
                // in its section, so a create from the Terminals section would
                // otherwise select a row that can sit below the fold.
                self.left_section = LeftSection::Projects;
                self.selected_left = self
                    .left_items()
                    .iter()
                    .position(|item| matches!(item, LeftItem::Session(index) if self.engine.sessions.get(*index).map(|candidate| candidate.id.as_str()) == Some(outcome.session.id.as_str())))
                    .unwrap_or(0);
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
                // open reconnect busy so its spinner does not linger (a create
                // launch commits unconditionally and never reaches
                // SessionMissing), then take down whatever launch spinner is
                // still on the line. An empty unkeyed message cannot do that job:
                // the line is a queue, so it retires an unkeyed entry and leaves
                // a keyed spinner up until the busy timeout calls it timed out.
                if let Some(op) = self.pending_reconnect_ops.remove(&outcome.session.id) {
                    self.apply_reaction(
                        op.resolve(&dux_core::engine::LaunchOutcome::Missing)
                            .into_reaction(),
                    );
                }
                self.status.retire_newest_busy();
            }
            AgentLaunchReadyView::Reconnect { status_message } => {
                self.show_agent_surface();
                // Land minimized unless the launch sought
                // fullscreen (see CreateCommitted above).
                self.land_completed_launch(outcome.wants_fullscreen);
                // Resolve the keyed reconnect op so its success replaces exactly
                // the "Launching…"/"Starting fresh…" busy. Keyed by tab id, which
                // the slot pointer names; an extra-tab launch has no op under its
                // tab id and falls back to an anonymous info rather than
                // resolving the slot tab's op with the wrong message. The engine's
                // message is shared with the web; the TUI appends where the launch
                // landed and how to toggle fullscreen.
                let status_message = self.launch_completion_message(
                    status_message.to_string(),
                    outcome.wants_fullscreen,
                );
                self.resolve_reconnect_op_or(
                    &outcome.tab_id,
                    dux_core::engine::LaunchOutcome::Ready {
                        status_message: status_message.into(),
                        quiet_on: outcome.status_quiet,
                    },
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
                    self.launch_completion_message(
                        status_message.to_string(),
                        outcome.wants_fullscreen,
                    )
                } else {
                    status_message.to_string()
                };
                self.resolve_reconnect_op_or(
                    &session_id,
                    dux_core::engine::LaunchOutcome::Ready {
                        status_message: status_message.into(),
                        quiet_on: outcome.status_quiet,
                    },
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
            // An extra tab has no op under its id, so this is the only path its
            // final takes; it honours the surface flag or a quieted launch would
            // still print here.
            dux_core::engine::Final::Message { quiet_on, .. } if quiet_on.tui => {
                self.status.retire_newest_busy();
            }
            dux_core::engine::Final::Message { tone, text, .. } => {
                self.status.set(std::time::Instant::now(), None, tone, text);
            }
            // Nothing to say, so the only job left is taking the spinner down.
            // The line is a queue, so this must retire the busy that is actually
            // on it: an empty unkeyed message would leave a KEYED spinner up
            // until the busy timeout mislabelled it as timed out.
            dux_core::engine::Final::Clear => {
                self.status.retire_newest_busy();
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

/// Whether a pruned PTY is the agent-level exit of `session_id`: the tab
/// holding the slot, or the tab that held it before this sweep and took the
/// agent's last live process with it. A clean exit hands the slot to a sibling
/// before this is read, so the pointer alone would lose the notice for exactly
/// the exit that ended the agent.
fn is_agent_exit_prune(
    engine: &Engine,
    pty: &PrunedPty,
    session_id: &str,
    slot_before_prune: Option<&str>,
) -> bool {
    if pty.kind != PrunedPtyKind::Agent || session_owner_id(pty) != Some(session_id) {
        return false;
    }
    engine.is_slot_tab_of(SessionIdRef::new(session_id), TabIdRef::new(&pty.id))
        || (pty.agent_detached && slot_before_prune == Some(pty.id.as_str()))
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

/// The status line for one pruned agent PTY, whichever kind of ending it had.
///
/// A REFUSED RESUME is its own sentence, built in `dux_core` so the terminal and
/// the browser quote the provider in the same words; only the closing remedy is
/// this surface's own, because it is the half that differs.
///
/// The remedy names both ways out, and it is deliberately not "press the
/// relaunch key to start fresh": that key resumes again whenever the provider is
/// eligible, which after a refusal is exactly what just failed. The fresh run is
/// the palette's `force-reconnect-agent`, which is palette-only and has no key of
/// its own to name.
fn pruned_agent_exit_message(pty: &PrunedPty, reconnect_key: &str) -> String {
    let remedy = format!(
        "Press \"{reconnect_key}\" to try again, or run force-reconnect-agent from the palette \
         to start a fresh session; the full output is in the agent's pane."
    );
    if let Some(warning) = pty.refused_resume_excerpt.as_deref().and_then(|excerpt| {
        dux_core::tab_verdict::refused_resume_warning(&pty.label, excerpt, &remedy)
    }) {
        return warning.to_string();
    }
    agent_exit_status_message(
        pty.exit_success,
        pty.is_minimal,
        &pty.output_excerpt,
        pty.read_error.as_deref(),
        reconnect_key,
    )
}

/// The ordinary exit status line for an agent PTY that has been pruned.
///
/// `read_error` is the one fact no other field carries. After a read error there
/// is almost never an exit status, so without it the message is identical to the
/// one a child that was merely slow to be reaped gets, and the user is told the
/// agent exited when what actually happened is that dux stopped being able to
/// read the terminal and killed a process that may have been perfectly healthy.
fn agent_exit_status_message(
    exit_success: Option<bool>,
    is_minimal: bool,
    excerpt: &str,
    read_error: Option<&str>,
    reconnect_key: &str,
) -> String {
    const MAX_EXIT_OUTPUT_CHARS: usize = 120;

    let output = excerpt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // What the provider left on screen, when there is a short enough run of it
    // to be worth quoting. Built once so the read-error message can carry it too.
    let output_clause = if is_minimal && !output.is_empty() {
        let output = truncate_status_output(&output, MAX_EXIT_OUTPUT_CHARS);
        let more = if output.truncated {
            " Full output was written to the logs."
        } else {
            ""
        };
        format!(" Output: {}.{more}", output.text)
    } else {
        String::new()
    };

    if let Some(error) = read_error {
        return format!(
            "Agent CLI process was killed after a terminal read error, so its exit status is \
             unknown: {error}.{output_clause} Press \"{reconnect_key}\" to relaunch."
        );
    }

    if output_clause.is_empty() {
        return format!("Agent CLI process has exited. Press \"{reconnect_key}\" to relaunch.");
    }
    let outcome = match exit_success {
        Some(false) => "exited with an error",
        Some(true) | None => "exited",
    };
    format!("Agent CLI process {outcome}.{output_clause} Press \"{reconnect_key}\" to relaunch.")
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

/// The warning for a reloaded `[server]` change that only a restart applies,
/// naming the changed `settings`.
pub(crate) fn server_restart_warning(serving_in_background: bool, settings: &[&str]) -> String {
    let named = settings.join(", ");
    match serving_in_background {
        true => format!(
            "Server settings changed in config ({named}), but a listener that is already bound \
             cannot adopt them. Stop the background server and start it again to apply them."
        ),
        false => format!(
            "Server settings changed in config ({named}). Nothing is serving right now, so they \
             apply the next time a server starts."
        ),
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

    /// An agent detaching is a fact about the workspace: a browser has always
    /// been told, and this surface used to speak only for the selected agent.
    #[test]
    fn an_agent_that_detaches_while_another_is_selected_still_says_so() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let pty = PrunedPty {
            kind: PrunedPtyKind::Agent,
            id: "other-slot".to_string(),
            owner: Some(dux_core::model::TerminalOwner::Session(
                "session-other".to_string(),
            )),
            agent_detached: true,
            label: "docs-pass".to_string(),
            tab_closed: false,
            exit_success: Some(false),
            is_minimal: false,
            output_excerpt: String::new(),
            read_error: None,
            refused_resume_excerpt: None,
            closed_tab: None,
        };

        app.apply_unselected_agent_exits(std::slice::from_ref(&pty), None);

        let (tone, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(tone, dux_core::statusline::StatusTone::Warning);
        assert_eq!(message, "Agent \"docs-pass\" exited.");

        // The selected agent's own richer line already said it, so the sweep
        // stays quiet rather than saying it a second time.
        let mut selected =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        selected.apply_unselected_agent_exits(std::slice::from_ref(&pty), Some("other-slot"));
        assert!(selected.status.most_recent_tui().is_none());
    }

    /// The web announces a reload before the drainer has applied it, so the
    /// apply's answer has to travel the lane both surfaces drain, on the key
    /// that premature sentence was raised under.
    #[test]
    fn a_failed_config_apply_answers_on_the_lane_both_surfaces_drain() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());

        app.post_config_reload_outcome(&TuiConfigReloadOutcome::ApplyFailed(
            "right_width_pct out of range".to_string(),
        ));

        let event = app.engine.worker_rx.try_recv().expect("a posted status");
        let WorkerEvent::PollerStatus(status) = event else {
            panic!("the apply outcome rides the poller-status lane");
        };
        assert_eq!(
            status.key.as_deref(),
            Some(dux_core::wire::status_keys::CONFIG_RELOAD)
        );
        assert_eq!(status.tone, dux_core::statusline::StatusTone::Error);
        assert!(status.message.contains("right_width_pct out of range"));

        // A validation failure never reached a browser, so it owes the lane
        // nothing and keeps this surface's own modal-and-error path.
        app.post_config_reload_outcome(&TuiConfigReloadOutcome::ValidationFailed);
        assert!(app.engine.worker_rx.try_recv().is_err());
    }

    fn test_session(worktree: &Path) -> AgentSession {
        AgentSession {
            id: "session-1".to_string(),
            slot_tab_id: "session-1-slot".to_string(),
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

    /// A closed tab says the same sentence here as in the browser, out of the
    /// prune's own account of the close: the row is gone, so the line cannot
    /// look the tab's provider up again.
    #[test]
    fn a_closed_tab_exit_says_the_shared_sentence_on_the_status_line() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let context = PruneViewContext::capture(&app);
        let mut pty = PrunedPty {
            kind: PrunedPtyKind::Agent,
            id: "extra".to_string(),
            owner: Some(dux_core::model::TerminalOwner::Session(
                "session-1".to_string(),
            )),
            agent_detached: false,
            label: "claude on feat".to_string(),
            tab_closed: true,
            exit_success: Some(true),
            is_minimal: false,
            output_excerpt: String::new(),
            read_error: None,
            refused_resume_excerpt: None,
            closed_tab: Some(dux_core::engine::ClosedTabExit {
                provider: "claude".to_string(),
                agent_label: "feat".to_string(),
                slot_provider: "codex".to_string(),
                tabs_remaining: 1,
            }),
        };

        app.apply_pruned_agent_tabs(std::slice::from_ref(&pty), &context);

        let (_, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(
            message,
            "Tab (claude) of agent \"feat\" exited cleanly and was closed; the pane now shows its \
             codex tab."
        );

        // A prune with no account of the close (an orphan PTY, whose session
        // nothing can name) says nothing rather than half a sentence.
        let mut fresh =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        pty.closed_tab = None;
        fresh.apply_pruned_agent_tabs(std::slice::from_ref(&pty), &context);
        assert!(
            fresh.status.most_recent_tui().is_none(),
            "an unattributable close names no tab and no agent"
        );
    }

    /// A promoted-away tab no longer holds the slot, so the notice that the
    /// agent is down has to follow the detachment rather than the pointer.
    #[test]
    fn a_detaching_tab_exit_is_the_agents_exit_whether_or_not_it_still_holds_the_slot() {
        let app = crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let exit = |id: &str, agent_detached: bool| PrunedPty {
            kind: PrunedPtyKind::Agent,
            id: id.to_string(),
            owner: Some(dux_core::model::TerminalOwner::Session(
                "session-1".to_string(),
            )),
            agent_detached,
            label: "agent".to_string(),
            tab_closed: true,
            exit_success: Some(true),
            is_minimal: false,
            output_excerpt: String::new(),
            read_error: None,
            refused_resume_excerpt: None,
            closed_tab: None,
        };

        let held_the_slot = Some("promoted-away");

        assert!(
            is_agent_exit_prune(
                &app.engine,
                &exit("promoted-away", true),
                "session-1",
                held_the_slot
            ),
            "the slot tab whose exit took the agent's last live process is the agent's exit"
        );
        assert!(
            !is_agent_exit_prune(
                &app.engine,
                &exit("promoted-away", false),
                "session-1",
                held_the_slot
            ),
            "a tab exiting while a sibling runs is a tab exit, not the agent's"
        );
        assert!(
            !is_agent_exit_prune(
                &app.engine,
                &exit("extra", true),
                "session-1",
                held_the_slot
            ),
            "an extra tab keeps its own dormant surface even when it detaches the agent"
        );
        assert!(
            is_agent_exit_prune(
                &app.engine,
                &exit("session-1-slot", false),
                "session-1",
                held_the_slot
            ),
            "the tab still holding the slot is the agent's own pane"
        );
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
            TabId::new("tab-x"),
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
        app.engine.providers.insert(TabId::new("tab-x"), client);
        // The exit these tests are about is a deliberate one, and `sh -c` is over
        // in milliseconds: without the input stamp the rapid-exit rule reads it as
        // a provider that never came up, which keeps the row for a reason that has
        // nothing to do with what is under test here.
        app.engine.note_pty_input("tab-x");
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
            !app.engine.providers.contains_key(TabIdRef::new("tab-x")),
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

    /// The TUI exit-prune teardown must clear EVERY runtime map keyed by the
    /// exited tab via the single-source `clear_tab_runtime`, not a
    /// hand-enumerated subset. A subset that drops providers, pins, activity and
    /// input still leaks `needs_attention`, `pty_progress`, and `agent_viewed`;
    /// on a long-running session that is one stranded entry per exited tab, and
    /// a stale attention or progress flag can resurface on a recycled id.
    #[test]
    fn exit_prune_clears_the_attention_progress_and_viewed_maps() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app
            .selected_session()
            .expect("test_app selects a session")
            .clone();
        let session_id = session.id.clone();
        // A clean-exiting session-slot provider, keyed by the tab id the
        // session's pointer names. That id is NOT the session id: keying these
        // maps on the session id would exercise the extra-tab branch and prove
        // nothing about the slot.
        let slot_tab_id = session.slot_tab_id().as_str().to_string();
        assert_ne!(slot_tab_id, session_id);
        assert!(app.engine.is_slot_tab(&session, session.slot_tab_id()));
        let client = crate::pty::PtyClient::spawn(
            "sh",
            &["-c".to_string(), "exit 0".to_string()],
            Path::new("."),
            10,
            40,
            100,
        )
        .expect("spawn pty");
        app.engine
            .providers
            .insert(TabId::new(slot_tab_id.clone()), client);
        // Seed the three runtime maps the teardown must clear.
        app.engine
            .needs_attention
            .insert(TabId::new(slot_tab_id.clone()));
        app.engine.pty_progress.insert(
            TabId::new(slot_tab_id.clone()),
            dux_core::pty::ProgressReport {
                working: true,
                at: std::time::Instant::now(),
            },
        );
        app.engine
            .agent_viewed
            .insert(TabId::new(slot_tab_id.clone()), std::time::Instant::now());

        // END OF INPUT *and* a reaped status: with either one missing the prune
        // is deliberately deferred inside REAPED_DRAIN_GRACE and one drain would
        // see nothing. See `wait_for_pty_eof`.
        crate::app::test_support::wait_for_pty_eof(&mut app, &slot_tab_id);
        app.drain_events();

        assert!(
            !app.engine
                .providers
                .contains_key(TabIdRef::new(&slot_tab_id)),
            "pruned"
        );
        assert!(
            !app.engine
                .needs_attention
                .contains(TabIdRef::new(&slot_tab_id)),
            "needs_attention must be cleared on exit prune"
        );
        assert!(
            !app.engine
                .pty_progress
                .contains_key(TabIdRef::new(&slot_tab_id)),
            "pty_progress must be cleared on exit prune"
        );
        assert!(
            !app.engine
                .agent_viewed
                .contains_key(TabIdRef::new(&slot_tab_id)),
            "agent_viewed must be cleared on exit prune"
        );
        // The SLOT branch of the teardown is the one that ran: an agent whose
        // slot provider exited is detached, which an extra tab's exit never does.
        assert_eq!(
            app.engine.sessions[0].status,
            dux_core::model::SessionStatus::Detached,
            "the exit must have been handled as the agent's slot tab exiting"
        );
    }

    /// A CLEAN exit (code 0) of the focused extra tab closes the tab itself:
    /// the user deliberately ended that conversation (e.g. /exit), so the row
    /// is deleted and, with no live sibling left, the pane minimizes and
    /// focus lands in the list, exactly like a single agent's clean exit.
    #[test]
    fn focused_extra_tab_clean_exit_closes_the_tab_and_minimizes() {
        let app = drain_focused_extra_tab_exit("0");
        assert!(
            !app.engine.agent_tabs.contains_key(TabIdRef::new("tab-x")),
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
    /// survives and the fullscreen overlay stays up, but interactive input
    /// still drops immediately so every escape hatch works.
    #[test]
    fn focused_extra_tab_crash_keeps_the_dormant_tab() {
        let app = drain_focused_extra_tab_exit("3");
        assert!(
            app.engine.agent_tabs.contains_key(TabIdRef::new("tab-x")),
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
                message: "creation failed".to_string().into(),
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

    /// A status quiet on the terminal UI never reaches the line, while one
    /// quiet on the web alone still does: the two surfaces decide separately.
    #[test]
    fn the_status_line_honours_the_terminal_half_of_the_quiet_flag() {
        use dux_core::statusline::QuietSurfaces;
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());

        app.apply_reaction(EventReaction::Status(
            StatusUpdate::info("withheld here").quiet_on(QuietSurfaces::TUI),
        ));
        assert!(
            app.status.most_recent_tui().is_none(),
            "a status quiet on the terminal UI must not take the line"
        );

        app.apply_reaction(EventReaction::Status(
            StatusUpdate::info("the web quiets this one").quiet_on(QuietSurfaces::WEB),
        ));
        let (_, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(
            message, "the web quiets this one",
            "quieting the web must not quiet the terminal UI"
        );
    }

    /// Withholding the sentence must not strand the spinner it was the answer
    /// to: a quieted KEYED final still retires its busy.
    #[test]
    fn a_quiet_keyed_final_still_retires_its_busy() {
        use crate::statusline::StatusTone;
        use dux_core::statusline::QuietSurfaces;
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        app.status.set(
            std::time::Instant::now(),
            Some("launch:/a".to_string()),
            StatusTone::Busy,
            "Launching...",
        );

        app.apply_reaction(EventReaction::Status(
            StatusUpdate::keyed("launch:/a", StatusTone::Info, "launched")
                .quiet_on(QuietSurfaces::BOTH),
        ));

        assert!(
            app.status
                .snapshot()
                .iter()
                .all(|s| s.key.as_deref() != Some("launch:/a")),
            "a quiet keyed final must take its spinner down"
        );
        assert!(app.status.most_recent_tui().is_none());
    }

    /// Only an info may be withheld. A warning reports something the screen
    /// cannot be standing in for, so the flag does not apply to it.
    #[test]
    fn a_warning_marked_quiet_still_takes_the_line() {
        use crate::statusline::StatusTone;
        use dux_core::statusline::QuietSurfaces;
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());

        app.apply_reaction(EventReaction::Status(
            StatusUpdate::warning("git is failing").quiet_on(QuietSurfaces::BOTH),
        ));

        let (tone, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(tone, StatusTone::Warning);
        assert_eq!(message, "git is failing");
    }

    /// A sticky busy waits for the user and refuses to be retired, so a quiet
    /// final against one is shown rather than withheld: a spinner nothing
    /// retires is worse than a line nobody needed.
    #[test]
    fn a_quiet_final_that_cannot_retire_its_busy_is_shown_instead() {
        use crate::statusline::StatusTone;
        use dux_core::statusline::QuietSurfaces;
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        app.status.set_scoped(
            std::time::Instant::now(),
            Some("launch:/a".to_string()),
            StatusTone::Busy,
            "Launching...",
            dux_core::statusline::StatusScope::All,
            true,
        );

        app.apply_reaction(EventReaction::Status(
            StatusUpdate::keyed("launch:/a", StatusTone::Info, "launched")
                .quiet_on(QuietSurfaces::BOTH),
        ));

        let (_, message) = app
            .status
            .most_recent_tui()
            .expect("the final must be shown rather than strand the spinner");
        assert_eq!(message, "launched");
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
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view: AgentLaunchReadyView::CreateCommitted {
                status_message: "Created agent.".to_string().into(),
                startup_result_error: None,
            },
        });

        assert!(
            app.status.snapshot().is_empty(),
            "the create View arm must not set any status; the engine emits the keyed final",
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
        app.engine
            .providers
            .insert(session.slot_tab_id().to_owned(), client);
        app.focus = FocusPane::Left;

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.slot_tab_id().to_string(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".to_string().into(),
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
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".to_string().into(),
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
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view: AgentLaunchReadyView::CreateCommitted {
                status_message: "Created agent.".to_string().into(),
                startup_result_error: None,
            },
        });

        assert_eq!(app.focus, FocusPane::Center);
        assert_eq!(app.input_target, InputTarget::None);
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::None);
    }

    /// A fresh agent is the list's selection AND on screen, even when the
    /// sidebar cursor was in the Terminals section and the agent lands at the
    /// bottom of a list taller than the pane: the agent list scrolls only to
    /// the selection of the section that has the cursor.
    #[test]
    fn create_committed_ready_scrolls_the_new_agent_into_view() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        app.engine.config.terminal.command = "cat".to_string();
        app.engine.config.terminal.args = vec![];
        let base = app.engine.sessions[0].clone();
        app.engine.sessions.clear();
        for index in 0..20 {
            let mut session = base.clone();
            session.id = format!("s{index:02}");
            session.title = Some(format!("agent {index:02}"));
            session.status = crate::model::SessionStatus::Active;
            app.engine.sessions.push(session);
        }
        app.rebuild_left_items();
        app.engine
            .create_standalone_terminal(24, 80)
            .expect("standalone terminal");
        app.selected_left = 0;
        app.left_section = LeftSection::Terminals;
        app.selected_terminal_index = 0;

        let newest = app.engine.sessions[19].clone();
        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: newest.id.clone(),
            session: newest.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view: AgentLaunchReadyView::CreateCommitted {
                status_message: "Created agent.".to_string().into(),
                startup_result_error: None,
            },
        });

        assert_eq!(app.left_section, LeftSection::Projects);
        assert_eq!(
            app.selected_session().map(|s| s.id.as_str()),
            Some(newest.id.as_str())
        );
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        assert!(
            app.mouse_layout
                .left_row_to_item
                .contains(&app.selected_left),
            "the new agent's row must be on screen; visible items {:?}, selected {}",
            app.mouse_layout.left_row_to_item,
            app.selected_left
        );
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
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view: AgentLaunchReadyView::ResumeFallback {
                session_id: session.id.clone(),
                status_message: "Fresh restart.".to_string().into(),
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
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".to_string().into(),
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
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Reconnected.".to_string().into(),
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

    /// End to end on the reconnect path: a resumed launch's busy is shown, its
    /// ready arrives quiet on both surfaces, and the line ends up empty with the
    /// spinner retired rather than left spinning behind a withheld sentence.
    #[test]
    fn a_quiet_reconnect_ready_leaves_the_line_empty_and_the_spinner_gone() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        let op = app.build_reconnect_status_op("Launching agent...".to_string());
        let op_key = op.id().to_string();
        app.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
        app.pending_reconnect_ops.insert(session.id.clone(), op);
        assert!(
            app.status.most_recent_tui().is_some(),
            "the busy must be on the line before the ready lands"
        );

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: session.id.clone(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            status_quiet: dux_core::statusline::QuietSurfaces::BOTH,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Resumed claude agent.".to_string().into(),
            },
        });

        assert!(
            app.status
                .snapshot()
                .iter()
                .all(|s| s.key.as_deref() != Some(op_key.as_str())),
            "the launch spinner must be gone"
        );
        assert!(
            app.status.most_recent_tui().is_none(),
            "a quiet ready must leave the line empty"
        );
        assert!(app.pending_reconnect_ops.is_empty());
    }

    /// The same, on the extra-tab path, which has no op stashed under its id and
    /// therefore takes the anonymous fallback rather than the keyed resolve.
    #[test]
    fn a_quiet_extra_tab_ready_says_nothing_on_the_line_either() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = app.engine.sessions[0].clone();

        app.apply_agent_launch_ready_view(AgentLaunchReadyOutcome {
            tab_id: "tab-extra".to_string(),
            session: session.clone(),
            pty_size: (80, 24),
            detached_session_id: None,
            wants_fullscreen: false,
            status_quiet: dux_core::statusline::QuietSurfaces::BOTH,
            view: AgentLaunchReadyView::Reconnect {
                status_message: "Resumed the claude conversation in this tab."
                    .to_string()
                    .into(),
            },
        });

        assert!(
            app.status.most_recent_tui().is_none(),
            "the unkeyed fallback must honour the surface flag too"
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

    /// A standing git failure in the background poller reaches the status line.
    /// It used to reach nothing at all here while the browser raised a warning
    /// about the same repository, which is two screens for one fact.
    #[test]
    fn a_standing_changed_files_failure_reaches_the_status_line() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let worktree = std::path::PathBuf::from("/tmp/wt-current");
        let session_id = app.engine.sessions[0].id.clone();
        *app.engine.watched_worktree.lock().expect("lock") = Some(worktree.clone());
        app.engine.watched_session_id = Some(session_id.clone());

        for _ in 0..dux_core::changes_status::ERROR_WARN_THRESHOLD {
            let reaction =
                app.engine
                    .process_worker_event(dux_core::worker::WorkerEvent::ChangedFilesReady {
                        outcome: Err("git status failed: index.lock exists".to_string()),
                        worktree: worktree.clone(),
                    });
            app.apply_reaction(reaction);
        }

        assert!(
            app.status.message().contains("temporarily unavailable"),
            "got {}",
            app.status.message()
        );
        assert!(
            app.status.snapshot().iter().any(|s| s.key.as_deref()
                == Some(dux_core::changes_status::warn_key(&session_id).as_str())),
            "the warning carries the shared key, so the recovery replaces it"
        );
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
            tab_id: session.slot_tab_id().to_owned(),
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
                status_message: "reconnect".to_string().into(),
            },
            wants_fullscreen: false,
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
        };

        dux_core::agent_job::run_agent_launch_job(request, worker_tx, &Default::default());

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

        let message = agent_exit_status_message(Some(false), true, &long_output, None, "r");

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
        let message = agent_exit_status_message(Some(false), true, "first\nsecond", None, "r");

        assert!(message.contains("Output: first second."));
        assert!(!message.contains('|'));
        assert!(!message.contains("Full output was written"));
    }

    /// A PTY that ended at a READ ERROR says so. The read error leaves no exit
    /// status, so without this the message is word for word the one a child that
    /// was merely slow to be reaped gets: the user is told the agent exited, when
    /// dux stopped being able to read the terminal and killed it.
    #[test]
    fn agent_exit_status_message_says_a_read_error_ended_the_run() {
        let message =
            agent_exit_status_message(None, true, "boot\nfailed", Some("bad file descriptor"), "r");

        assert!(
            message.contains("read error"),
            "the one fact no other field carries must be in the line: {message}"
        );
        assert!(
            message.contains("bad file descriptor"),
            "the error itself is what makes the line actionable: {message}"
        );
        assert!(
            message.contains("Output: boot failed."),
            "what the provider managed to print is still worth showing: {message}"
        );
        assert!(message.contains("to relaunch"));

        let ordinary = agent_exit_status_message(None, true, "boot\nfailed", None, "r");
        assert!(
            !ordinary.contains("read error"),
            "an ordinary end of input must not gain a read-error clause: {ordinary}"
        );
    }

    /// A pruned agent whose exit detached it, with no refusal recorded.
    fn pruned_agent(label: &str) -> PrunedPty {
        PrunedPty {
            kind: PrunedPtyKind::Agent,
            id: "s1-slot".to_string(),
            owner: Some(dux_core::model::TerminalOwner::Session("s1".to_string())),
            agent_detached: true,
            label: label.to_string(),
            tab_closed: false,
            exit_success: Some(false),
            is_minimal: true,
            output_excerpt: "boom".to_string(),
            read_error: None,
            refused_resume_excerpt: None,
            closed_tab: None,
        }
    }

    /// A refused resume tells the user what the provider said and how to get out
    /// of it. Without it the line says only that the agent exited, and the words
    /// that explain why leave the screen with the pane.
    #[test]
    fn a_refused_resume_status_quotes_the_provider_and_names_the_relaunch_key() {
        let mut pty = pruned_agent("feat/x");
        pty.refused_resume_excerpt = Some(vec![
            "Resuming your conversation.".to_string(),
            "Looking for a session to continue.".to_string(),
            "Found session 9f2 for this directory.".to_string(),
            "That session cannot be continued here.".to_string(),
            "Your most recent conversation is running in the background.".to_string(),
            "Use `claude agents` to attach to it.".to_string(),
        ]);

        assert_eq!(
            pruned_agent_exit_message(&pty, "Ctrl-r"),
            "Agent \"feat/x\" could not resume its previous session; the provider said: \u{2026} \
             Your most recent conversation is running in the background. Use `claude agents` to \
             attach to it. Press \"Ctrl-r\" to try again, or run force-reconnect-agent from the \
             palette to start a fresh session; the full output is in the agent's pane."
        );
    }

    /// An ordinary exit keeps the wording it had.
    #[test]
    fn an_ordinary_agent_exit_status_is_unchanged() {
        let message = pruned_agent_exit_message(&pruned_agent("feat/x"), "Ctrl-r");
        assert_eq!(
            message,
            agent_exit_status_message(Some(false), true, "boom", None, "Ctrl-r"),
            "with no refusal recorded the line is the ordinary exit message"
        );
        assert!(!message.contains("could not resume"));
    }

    #[test]
    fn fork_worker_requires_name_from_prompt() {
        let tmp = tempdir().expect("tempdir");
        let paths = DuxPaths {
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            socket_path: tmp.path().join("dux.sock"),
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
            slot_tab_id: "session-1-slot".to_string(),
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
            dux_core::test_provider::harmless_config(),
            worker_tx,
            (80, 24),
            "op-test".to_string(),
            dux_core::term_identity::TerminalIdentity::default(),
            Default::default(),
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
        // A client that asked for this reload waits on its record.
        app.engine.open_operation(
            "reload-op",
            dux_core::operations::OperationKind::ConfigReload,
        );
        app.engine.operations.await_reload("reload-op", false);
        app.engine.operations.reload_closed();

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));
        app.drain_worker_events();

        let (tone, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(tone, StatusTone::Warning, "the last word is the warning");
        assert!(
            message.contains("server"),
            "the warning names what needs restarting: {message}"
        );
        let record = app
            .engine
            .operations
            .peek("reload-op", std::time::Instant::now())
            .expect("the record is kept");
        assert_eq!(
            record.state,
            dux_core::operations::OperationState::Succeeded
        );
        assert!(
            record.message.starts_with(
                "Configuration reloaded. New settings are active now. Server settings changed"
            ),
            "{}",
            record.message
        );
    }

    #[test]
    fn a_reload_that_leaves_the_server_section_alone_warns_about_nothing() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let mut config = app.engine.config.clone();
        config.ui.diff_tab_width += 1;

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));
        app.drain_worker_events();

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
        app.drain_worker_events();

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
    fn a_serving_terminal_ui_gets_the_stop_and_start_wording_on_a_startup_bound_change() {
        // A bind setting and each of the four server log settings: the log is
        // opened when the serve starts, so a running one cannot adopt them.
        type Change = fn(&mut dux_core::config::ServerConfig);
        let changes: [(&str, Change); 15] = [
            ("host", |server| server.host = "0.0.0.0".into()),
            ("port", |server| server.port += 1),
            ("max_websocket_events_connections", |server| {
                server.max_websocket_events_connections += 1
            }),
            ("max_websocket_agent_connections", |server| {
                server.max_websocket_agent_connections += 1
            }),
            ("max_websocket_terminal_connections", |server| {
                server.max_websocket_terminal_connections += 1
            }),
            ("max_websocket_tab_connections", |server| {
                server.max_websocket_tab_connections += 1
            }),
            ("max_websocket_tabs_per_agent", |server| {
                server.max_websocket_tabs_per_agent += 1
            }),
            ("file_drop_max_bytes", |server| {
                server.file_drop_max_bytes += 1
            }),
            ("file_drop_max_concurrency", |server| {
                server.file_drop_max_concurrency += 1
            }),
            ("tree_list_max_concurrency", |server| {
                server.tree_list_max_concurrency += 1
            }),
            ("release_notes_max_concurrency", |server| {
                server.release_notes_max_concurrency += 1
            }),
            ("log_path", |server| server.log_path = "other.log".into()),
            ("log_max_bytes", |server| server.log_max_bytes += 1),
            ("log_keep", |server| server.log_keep += 1),
            ("log_compress", |server| {
                server.log_compress = !server.log_compress
            }),
        ];
        for (setting, change) in changes {
            let (companion, _recorded) =
                crate::app::background_server::tests::FakeCompanion::serving();
            let mut app =
                crate::app::test_support::test_app(crate::app::test_support::default_bindings());
            app.engine.config.server.serve_while_tui = true;
            app.companion = Some(companion);
            let mut config = app.engine.config.clone();
            // Kept on, or the reload's own live switch stops the serve before the
            // warning is chosen and the copy would be right for the wrong reason.
            config.server.serve_while_tui = true;
            change(&mut config.server);

            app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

            let (tone, message) = app.status.most_recent_tui().expect("a status");
            assert_eq!(tone, StatusTone::Warning, "{setting}");
            assert!(
                message.contains(&format!("({setting})")),
                "{setting}: the warning names what changed: {message}"
            );
            assert!(
                message.contains("Stop the background server and start it again"),
                "{setting}: a serving companion picks the stop-and-start copy: {message}"
            );
        }
    }

    /// A config the engine adopted after its own apply failed runs the same
    /// comparison a successful reload does: a bind change still owes the
    /// restart warning.
    #[test]
    fn an_adopted_config_runs_the_reload_comparison() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let before = app.engine.config.clone();
        app.engine.config.server.port += 1;

        app.apply_reaction(EventReaction::ConfigAdopted {
            github_was_enabled: before.ui.github_integration,
            before: Box::new(before),
            error: "the session database could not be read".to_string(),
        });

        let (_, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(message, server_restart_warning(false, &["port"]));
    }

    #[test]
    fn an_idle_terminal_ui_gets_the_next_start_wording_on_a_bind_or_socket_change() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        assert!(!app.background_server_is_serving());
        let mut config = app.engine.config.clone();
        config.server.port += 1;

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

        let (_, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(message, server_restart_warning(false, &["port"]));
        assert_eq!(
            message,
            "Server settings changed in config (port). Nothing is serving right now, so they apply the next time a server starts."
        );

        // The control socket is bound once per process: a new path waits for
        // dux itself to start again, serving or not.
        let mut config = app.engine.config.clone();
        config.server.control_socket = "/run/user/1000/dux.sock".to_string();

        app.apply_reaction(EventReaction::ApplyReloadedConfig(Box::new(config)));

        let (tone, message) = app.status.most_recent_tui().expect("a status");
        assert_eq!(tone, StatusTone::Warning);
        assert!(message.contains("control_socket"), "{message}");
        assert!(message.contains("next time dux starts"), "{message}");
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
        assert_eq!(message, server_restart_warning(false, &["port"]));
    }

    #[test]
    fn the_server_restart_warning_names_the_background_server_only_while_it_serves() {
        let serving = server_restart_warning(true, &["port"]);
        let idle = server_restart_warning(false, &["port"]);
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
            socket_path: tmp.path().join("dux.sock"),
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
                cloned: None,
            },
            paths,
            dux_core::test_provider::harmless_config(),
            worker_tx,
            (80, 24),
            "op-create-1".to_string(),
            dux_core::term_identity::TerminalIdentity::default(),
            Default::default(),
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

    /// A launch that ends with nothing to say still has to take its spinner
    /// down, and the spinner it has to take down is usually KEYED. Writing an
    /// empty unkeyed message did that back when the line was most-recent-wins;
    /// against a keyed busy on a queued line it does nothing at all, and the
    /// spinner sits there until the busy timeout calls it timed out, which it
    /// was not.
    #[test]
    fn a_launch_with_nothing_to_say_takes_the_keyed_spinner_down() {
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        app.status.set(
            std::time::Instant::now(),
            Some("reconnect:session-1".to_string()),
            StatusTone::Busy,
            "Reconnecting\u{2026}",
        );
        assert_eq!(app.status.tone(), StatusTone::Busy);

        app.resolve_reconnect_op_or("session-1", dux_core::engine::LaunchOutcome::Missing);

        assert!(
            app.status.most_recent_tui().is_none(),
            "the spinner must be gone, not waiting on the busy timeout: {:?}",
            app.status.most_recent_tui()
        );
    }

    /// The reaper's detach outcome has to travel the whole way to the status
    /// line. Everything before this seam is engine state nobody sees; a final
    /// the seam drops leaves a spinner up until the busy timeout guesses at it.
    #[test]
    fn a_detach_outcome_reaches_the_status_line() {
        let worktree = tempdir().expect("worktree");
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let session = test_session(worktree.path());
        let session_id = session.id.clone();
        app.engine.sessions.push(session);
        app.engine.providers.insert(
            dux_core::ids::TabId::new(format!("{session_id}-slot")),
            crate::pty::PtyClient::spawn("cat", &[], worktree.path(), 24, 80, 1000)
                .expect("spawn provider"),
        );

        let dux_core::engine::DetachSessionOutcome::Started { busy, .. } =
            app.engine.begin_detach_session(&session_id)
        else {
            panic!("a live agent detaches");
        };
        app.status.set(
            std::time::Instant::now(),
            Some(dux_core::engine::detach_status_key(&session_id)),
            dux_core::statusline::StatusTone::Busy,
            busy,
        );
        assert_eq!(
            app.status.tone(),
            dux_core::statusline::StatusTone::Busy,
            "the spinner is up while the child is being waited out"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.status.tone() == dux_core::statusline::StatusTone::Busy {
            app.apply_reaped_terminations();
            assert!(
                std::time::Instant::now() < deadline,
                "the detach outcome never reached the status line"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            app.status.text().contains("shut down and is now detached"),
            "the outcome replaces the spinner in words: {}",
            app.status.text()
        );
    }

    /// A detach the user ASKED for is not an agent falling over, and must not be
    /// announced as one while a different agent is selected.
    ///
    /// The two travel different roads (a detach moves its processes to the
    /// terminating set, so the exit prune never sees them), and this is what
    /// keeps them apart: one deliberate act, one sentence.
    #[test]
    fn a_deliberate_detach_of_an_unselected_agent_says_only_that_it_detached() {
        let worktree = tempdir().expect("worktree");
        let mut app =
            crate::app::test_support::test_app(crate::app::test_support::default_bindings());
        let selected = app
            .selected_session()
            .expect("test_app selects an agent")
            .id
            .clone();
        let mut other = test_session(worktree.path());
        other.id = "session-2".to_string();
        other.slot_tab_id = "session-2-slot".to_string();
        other.title = Some("the other agent".to_string());
        app.engine.sessions.push(other);
        assert_ne!(
            selected, "session-2",
            "the detached agent is not the selected one"
        );
        app.engine.providers.insert(
            dux_core::ids::TabId::new("session-2-slot"),
            crate::pty::PtyClient::spawn("cat", &[], worktree.path(), 24, 80, 1000)
                .expect("spawn provider"),
        );

        let dux_core::engine::DetachSessionOutcome::Started { busy, .. } =
            app.engine.begin_detach_session("session-2")
        else {
            panic!("a live agent detaches");
        };
        assert!(
            !app.engine
                .providers
                .contains_key(dux_core::ids::TabIdRef::new("session-2-slot")),
            "the detach takes the process out of the set the exit prune reads"
        );
        app.status.set(
            std::time::Instant::now(),
            Some(dux_core::engine::detach_status_key("session-2")),
            dux_core::statusline::StatusTone::Busy,
            busy,
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.status.tone() == dux_core::statusline::StatusTone::Busy {
            app.apply_reaped_terminations();
            app.apply_pruned_pty_events();
            assert!(
                !app.status.text().contains("exited"),
                "a detach the user asked for is not an exit to report: {}",
                app.status.text()
            );
            assert!(
                std::time::Instant::now() < deadline,
                "the detach outcome never reached the status line"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            app.status.text().contains("detached"),
            "the one sentence is the detach's own: {}",
            app.status.text()
        );
    }
}
