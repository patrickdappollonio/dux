//! Managing a project from the terminal UI: `manage-projects`' action list, the
//! Project info screen, and Change base branch.
//!
//! `manage-projects` opens the project list; a pick opens that project's action
//! list ([`PromptState::ProjectActions`]), whose rows follow the browser's
//! project `⋯` menu. Every row runs the existing flow for that project, passed
//! in explicitly: there is no hidden "target project" any more. A project
//! palette command run on its own acts on the selected agent's project, or,
//! with no agent selected, opens the project list and runs on the pick
//! ([`App::run_project_command`]).
//!
//! Anything opened from the action list (a confirmation, Project info, a
//! settings editor, the worktree manager, the startup logs) carries it in its
//! `return_to`, so closing it by any way out steps back to the list, the way the
//! browser's dialogs close back onto its Projects list.

use std::sync::mpsc::TryRecvError;

use super::modal::{ModalKeyStep, binding_lookup_is_suppressed, modal_key_step};
use super::*;
use crate::keybindings::BindingScope;
use dux_core::git::BranchChoice;

/// Name who holds each held branch: the agent whose folder is the holder, when
/// one is, compared on canonical paths so a symlinked spelling still matches.
/// Runs in the listing worker, never on the UI thread, because it reads the
/// filesystem.
pub(crate) fn resolve_branch_holders(
    branches: &[BranchChoice],
    agents: &[(String, String)],
) -> HashMap<String, BranchHolder> {
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let agents: Vec<(&str, PathBuf)> = agents
        .iter()
        .map(|(label, dir)| (label.as_str(), canonical(Path::new(dir))))
        .collect();
    branches
        .iter()
        .filter_map(|branch| {
            let holder = branch.held_by.as_ref()?;
            let holder_canonical = canonical(holder);
            let agent = agents
                .iter()
                .find(|(_, dir)| *dir == holder_canonical)
                .map(|(label, _)| (*label).to_string());
            Some((
                branch.name.clone(),
                BranchHolder {
                    agent,
                    path: holder.display().to_string(),
                },
            ))
        })
        .collect()
}

/// Take the action list a project configure editor was opened from.
pub(crate) fn configure_return_to(prompt: &mut PromptState) -> Option<Box<ProjectActionsPrompt>> {
    match prompt {
        PromptState::ConfigureStartupCommand { return_to, .. }
        | PromptState::ConfigureProjectEnv { return_to, .. } => return_to.take(),
        _ => None,
    }
}

/// The Change base branch picker's `/`-search predicate: a case-insensitive
/// substring of the branch name. `needle` is expected pre-lowercased.
pub(crate) fn branch_choice_matches(branch: &BranchChoice, needle: &str) -> bool {
    branch.name.to_lowercase().contains(needle)
}

impl App {
    /// A project palette command: run `action` on the selected agent's
    /// project, or, with no agent selected (or an agent with no project),
    /// open the project list and run it on the pick.
    pub(crate) fn run_project_command(&mut self, action: ProjectAction) -> Result<()> {
        match self.selected_project().cloned() {
            Some(project) => self.run_project_action(&project, action, None),
            None => self.open_project_chooser(ProjectChooserIntent::Action(action)),
        }
    }

    /// Run `action` on `project` through its existing flow. `return_to` is the
    /// action list the action was picked from, which a confirmation raised by
    /// it steps back to on cancel. A refusal leaves whatever prompt is open in
    /// place, so a refused row keeps the user on the list that offered it.
    pub(crate) fn run_project_action(
        &mut self,
        project: &Project,
        action: ProjectAction,
        return_to: Option<Box<ProjectActionsPrompt>>,
    ) -> Result<()> {
        match action {
            ProjectAction::NewAgent => self.begin_new_agent_for_project(project.clone()),
            ProjectAction::NewAgentFromPr => {
                if !self.github_pr_agent_command_available() {
                    self.set_error(
                        "GitHub PR agent creation requires GitHub integration and an \
                         authenticated gh CLI.",
                    );
                    return Ok(());
                }
                self.begin_pr_agent_for_project(project.clone())
            }
            ProjectAction::Worktrees => {
                self.begin_manage_worktrees_for_project(project.clone(), return_to)
            }
            ProjectAction::NewTerminal => {
                self.prompt = PromptState::None;
                self.show_project_terminal(project)
            }
            ProjectAction::Pull => self.pull_project(project),
            ProjectAction::CheckoutDefaultBranch => {
                self.ask_checkout_project_default_branch(project, return_to);
                Ok(())
            }
            ProjectAction::ChangeBaseBranch => {
                self.open_change_base_branch(project, return_to);
                Ok(())
            }
            ProjectAction::Info => {
                self.open_project_info(project, return_to);
                Ok(())
            }
            ProjectAction::DefaultProvider => {
                self.open_change_project_default_provider_for(project, return_to)
            }
            ProjectAction::AutoReopen => self.toggle_project_auto_reopen_agents_for(project),
            ProjectAction::StartupCommand => {
                self.open_configure_startup_command_for(project, return_to)
            }
            ProjectAction::Environment => self.open_configure_project_env_for(project, return_to),
            ProjectAction::StartupLogs => {
                // The logs open in their own modal once they are read, over
                // whatever is open then: the action list stays up meanwhile
                // (a project with no runs yet answers on the status line), and
                // the arriving logs take it as the place to step back to.
                self.open_project_startup_command_logs(project);
                Ok(())
            }
            ProjectAction::Delete => {
                self.confirm_delete_project(project, return_to);
                Ok(())
            }
            ProjectAction::Remove => {
                self.confirm_remove_project(project, return_to);
                Ok(())
            }
            ProjectAction::CopyPath => {
                self.copy_project_path(project);
                Ok(())
            }
        }
    }

    /// Open the action list for `target`. `return_to` is the project list as
    /// the user left it, which Escape steps back to.
    pub(crate) fn open_project_actions(
        &mut self,
        target: ProjectActionsTarget,
        return_to: Option<SearchableList>,
    ) {
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ProjectActions(ProjectActionsPrompt {
            action: None,
            target,
            selected: 0,
            return_to,
        });
    }

    /// The rows of `target`'s action list, derived from the live project so a
    /// folder that goes missing, or an auto-reopen that flips, shows at once.
    /// An orphaned group offers Remove project… alone; a target that no longer
    /// exists offers nothing.
    pub(crate) fn project_action_rows(
        &self,
        target: &ProjectActionsTarget,
    ) -> Vec<ProjectActionRow> {
        let project = match target {
            ProjectActionsTarget::Orphaned { .. } => {
                return vec![ProjectActionRow {
                    action: ProjectAction::Remove,
                    label: ProjectAction::Remove.label().to_string(),
                    unavailable: None,
                }];
            }
            ProjectActionsTarget::Project { id } => {
                match self.engine.projects.iter().find(|p| &p.id == id) {
                    Some(project) => project,
                    None => return Vec::new(),
                }
            }
        };
        let gh = self.github_pr_agent_command_available();
        ProjectAction::ROWS
            .into_iter()
            .filter(|action| gh || *action != ProjectAction::NewAgentFromPr)
            .map(|action| {
                let label = match action {
                    ProjectAction::AutoReopen => {
                        if self.engine.project_allows_auto_reopen(&project.id) {
                            "Turn off auto-reopen agents".to_string()
                        } else {
                            "Turn on auto-reopen agents".to_string()
                        }
                    }
                    other => other.label().to_string(),
                };
                let unavailable = (action.needs_folder() && project.path_missing).then(|| {
                    format!(
                        "{} is unavailable: the folder of project \"{}\" is missing at {}.",
                        action.chooser_title(),
                        project.name,
                        project.path
                    )
                });
                ProjectActionRow {
                    action,
                    label,
                    unavailable,
                }
            })
            .collect()
    }

    /// Whether the thing an action list is about still exists: the project
    /// record, or, for an orphaned group, agents with no record behind them.
    pub(crate) fn project_actions_target_exists(&self, target: &ProjectActionsTarget) -> bool {
        match target {
            ProjectActionsTarget::Project { id } => {
                self.engine.projects.iter().any(|p| &p.id == id)
            }
            ProjectActionsTarget::Orphaned { project_id, .. } => {
                !self.engine.projects.iter().any(|p| &p.id == project_id)
                    && self.project_agent_count(project_id) > 0
            }
        }
    }

    /// Step back to the action list a dialog was raised from, when there was
    /// one and its project still exists. A project that is gone leaves nothing
    /// to step back to, so the stack simply closes.
    pub(crate) fn return_to_project_actions(
        &mut self,
        return_to: Option<Box<ProjectActionsPrompt>>,
    ) {
        let Some(prompt) = return_to else {
            return;
        };
        if self.project_actions_target_exists(&prompt.target) {
            self.prompt = PromptState::ProjectActions(*prompt);
        }
    }

    /// Escape on the action list: back to the project list it was opened from,
    /// with the same search and the same project under the cursor, or closed
    /// when it was opened on its own.
    pub(super) fn leave_project_actions(&mut self) {
        let PromptState::ProjectActions(prompt) = &self.prompt else {
            return;
        };
        let target_id = match &prompt.target {
            ProjectActionsTarget::Project { id } => id.clone(),
            ProjectActionsTarget::Orphaned { project_id, .. } => project_id.clone(),
        };
        let Some(mut list) = prompt.return_to.clone() else {
            self.prompt = PromptState::None;
            return;
        };
        let mut entries = self.build_project_chooser_entries();
        entries.extend(self.build_orphaned_group_entries());
        if entries.is_empty() {
            self.prompt = PromptState::None;
            return;
        }
        let visible = list.visible_indices(&entries, pick_project_matches);
        match visible
            .iter()
            .position(|index| entries[*index].id == target_id)
        {
            Some(position) => list.selected = position,
            None => list.clamp_selected(visible.len()),
        }
        self.prompt = PromptState::PickProject {
            intent: ProjectChooserIntent::Manage,
            entries,
            list,
        };
    }

    /// Where the action list's cursor is among `rows`: on the action it was
    /// on when that action is still a row, else at its last index, clamped.
    pub(crate) fn project_actions_cursor(
        prompt: &ProjectActionsPrompt,
        rows: &[ProjectActionRow],
    ) -> usize {
        prompt
            .action
            .and_then(|action| rows.iter().position(|row| row.action == action))
            .unwrap_or(prompt.selected)
            .min(rows.len().saturating_sub(1))
    }

    /// Put the action list's cursor on row `index` of `rows`.
    fn place_project_actions_cursor(&mut self, index: usize, rows: &[ProjectActionRow]) {
        if let PromptState::ProjectActions(prompt) = &mut self.prompt
            && let Some(row) = rows.get(index)
        {
            prompt.selected = index;
            prompt.action = Some(row.action);
        }
    }

    /// Run the action list's selected row. An unavailable row says why and runs
    /// nothing.
    pub(crate) fn activate_project_action_row(&mut self) -> Result<()> {
        let PromptState::ProjectActions(prompt) = &self.prompt else {
            return Ok(());
        };
        let mut prompt = prompt.clone();
        let rows = self.project_action_rows(&prompt.target);
        let cursor = Self::project_actions_cursor(&prompt, &rows);
        let Some(row) = rows.get(cursor).cloned() else {
            return Ok(());
        };
        prompt.selected = cursor;
        prompt.action = Some(row.action);
        if let Some(reason) = row.unavailable {
            self.set_warning(reason);
            return Ok(());
        }
        let return_to = Some(Box::new(prompt.clone()));
        match &prompt.target {
            ProjectActionsTarget::Project { id } => {
                let Some(project) = self.engine.projects.iter().find(|p| &p.id == id).cloned()
                else {
                    self.prompt = PromptState::None;
                    self.set_error("That project is no longer available.");
                    return Ok(());
                };
                self.run_project_action(&project, row.action, return_to)
            }
            ProjectActionsTarget::Orphaned { project_id, name } => {
                self.confirm_remove_orphaned_group(project_id.clone(), name.clone(), return_to);
                Ok(())
            }
        }
    }

    pub(super) fn handle_project_actions_prompt_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<bool>> {
        let PromptState::ProjectActions(prompt) = &self.prompt else {
            return Ok(None);
        };
        let rows = self.project_action_rows(&prompt.target);
        let cursor = Self::project_actions_cursor(prompt, &rows);
        let action = self
            .bindings
            .lookup(&key, BindingScope::Palette)
            .or_else(|| self.bindings.lookup(&key, BindingScope::Dialog));
        match action {
            Some(Action::CloseOverlay) => self.leave_project_actions(),
            Some(Action::MoveDown) if cursor + 1 < rows.len() => {
                self.place_project_actions_cursor(cursor + 1, &rows);
            }
            Some(Action::MoveUp) if cursor > 0 => {
                self.place_project_actions_cursor(cursor - 1, &rows);
            }
            Some(Action::Confirm) => self.activate_project_action_row()?,
            _ => {}
        }
        Ok(Some(false))
    }

    /// A click on the action list's `index`th row selects it; a double click
    /// runs it, like every other picker.
    pub(super) fn click_project_action_row(&mut self, index: usize, double_click: bool) {
        let rows = match &self.prompt {
            PromptState::ProjectActions(prompt) => self.project_action_rows(&prompt.target),
            _ => return,
        };
        self.place_project_actions_cursor(index, &rows);
        if double_click && let Err(err) = self.activate_project_action_row() {
            self.set_error(format!("{err:#}"));
        }
    }

    // ── Project info ───────────────────────────────────────────────────────

    /// Open the read-only Project info screen for `project`: the facts the
    /// browser's Project info dialog shows.
    pub(crate) fn open_project_info(
        &mut self,
        project: &Project,
        return_to: Option<Box<ProjectActionsPrompt>>,
    ) {
        let rows = self.project_info_rows(project);
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ProjectInfo(ProjectInfoPrompt {
            project_name: project.name.clone(),
            rows,
            return_to,
        });
    }

    /// The Project info facts, in the browser dialog's order.
    pub(crate) fn project_info_rows(&self, project: &Project) -> Vec<ProjectInfoRow> {
        let name = |label, value: String| ProjectInfoRow {
            label,
            value,
            is_name: true,
        };
        let words = |label, value: String| ProjectInfoRow {
            label,
            value,
            is_name: false,
        };
        let branch = project.current_branch.trim();
        let session_ids: HashSet<&str> = self
            .engine
            .sessions
            .iter()
            .filter(|session| session.project_id() == Some(project.id.as_str()))
            .map(|session| session.id.as_str())
            .collect();
        let terminals = self
            .engine
            .companion_terminals
            .values()
            .filter(|terminal| match &terminal.owner {
                TerminalOwner::Session(id) => session_ids.contains(id.as_str()),
                TerminalOwner::Project(id) => id == &project.id,
                TerminalOwner::Standalone => false,
            })
            .count();
        let provider = if project.explicit_default_provider.is_some() {
            format!("{} (explicit)", project.default_provider.as_str())
        } else {
            project.default_provider.as_str().to_string()
        };
        vec![
            name("Path", project.path.clone()),
            if branch.is_empty() {
                words("Current branch", "Unknown".to_string())
            } else {
                name("Current branch", branch.to_string())
            },
            match &project.leading_branch {
                Some(base) => name("Base branch", base.clone()),
                None => words("Base branch", "No base recorded yet".to_string()),
            },
            words(
                "Added",
                project
                    .created_at
                    .map(|at| at.format("%b %-d, %Y").to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
            ),
            name("Default provider", provider),
            words(
                "Auto-reopen",
                match project.auto_reopen_agents {
                    None => "Inherit",
                    Some(true) => "On",
                    Some(false) => "Off",
                }
                .to_string(),
            ),
            match project.startup_command.as_deref().map(str::trim) {
                Some(command) if !command.is_empty() => {
                    name("Startup command", command.to_string())
                }
                _ => words("Startup command", "None".to_string()),
            },
            words(
                "Environment",
                dux_core::text::count_of(project.env.len(), "variable"),
            ),
            words(
                "Live agents",
                dux_core::text::count_of(session_ids.len(), "agent"),
            ),
            words(
                "Companion terminals",
                dux_core::text::count_of(terminals, "terminal"),
            ),
        ]
    }

    /// Close the Project info screen, back onto the action list it came from.
    pub(crate) fn close_project_info(&mut self) {
        let PromptState::ProjectInfo(prompt) = &self.prompt else {
            return;
        };
        let return_to = prompt.return_to.clone();
        self.prompt = PromptState::None;
        self.return_to_project_actions(return_to);
    }

    pub(super) fn handle_project_info_prompt_key(&mut self, key: KeyEvent) -> Option<bool> {
        if !matches!(self.prompt, PromptState::ProjectInfo(_)) {
            return None;
        }
        let action = self.bindings.lookup(&key, BindingScope::Dialog);
        if matches!(action, Some(Action::Confirm | Action::CloseOverlay))
            || key.code == KeyCode::Char(' ')
        {
            self.close_project_info();
        }
        Some(false)
    }

    // ── Change base branch ─────────────────────────────────────────────────

    /// `change-project-base-branch`: for the selected agent's project, or for
    /// one picked from the project list first.
    pub(crate) fn change_selected_project_base_branch(&mut self) -> Result<()> {
        self.run_project_command(ProjectAction::ChangeBaseBranch)
    }

    /// Open the Change base branch picker for `project` and list its branches
    /// in a worker, after one bounded fetch of origin. The picker shows a
    /// loading row until the answer lands in [`App::drain_branch_listing`].
    pub(crate) fn open_change_base_branch(
        &mut self,
        project: &Project,
        return_to: Option<Box<ProjectActionsPrompt>>,
    ) {
        if project.path_missing {
            self.set_warning(
                dux_core::engine::change_base_branch_path_missing_message(&project.name)
                    .message()
                    .to_string(),
            );
            return;
        }
        // One listing at a time: an earlier one's answer would be about a
        // picker that is no longer open, so its busy ends here.
        if let Some(previous) = self.pending_branch_listing.take() {
            self.apply_reaction(
                previous
                    .op
                    .resolve(&BranchListingOutcome::Dismissed)
                    .into_reaction(),
            );
        }
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ChangeBaseBranch(Box::new(ChangeBaseBranchPrompt {
            holders: std::collections::HashMap::new(),
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            current_base: project.leading_branch.clone(),
            loading: true,
            branches: Vec::new(),
            fetch_note: None,
            error: None,
            list: SearchableList::new(),
            return_to,
        }));

        let failed_name = project.name.clone();
        let op = dux_core::engine::status_op(format!(
            "Fetching origin and listing the branches of project \"{}\"...",
            project.name
        ))
        .resolve_in_handler(move |outcome: &BranchListingOutcome| match outcome {
            // The list on screen is the answer.
            BranchListingOutcome::Loaded | BranchListingOutcome::Dismissed => {
                dux_core::engine::Final::clear()
            }
            BranchListingOutcome::Failed(error) => dux_core::engine::Final::error(format!(
                "Could not list the branches of project \"{failed_name}\": {error}. Close the \
                 list and open Change base branch again to retry."
            )),
        });
        let pending = self.engine.begin_status_op(&op);
        let (tx, rx) = mpsc::channel();
        let project_id = project.id.clone();
        let path = PathBuf::from(&project.path);
        // The agents as they are now, labels and folders, so the worker can say
        // which agent holds a branch without the UI thread touching disk.
        let agents: Vec<(String, String)> = self
            .engine
            .sessions
            .iter()
            .map(|session| (session.display_label(), session.directory().to_string()))
            .collect();
        thread::spawn(move || {
            let answer = std::panic::catch_unwind(|| {
                let result = dux_core::base_branch::load_branch_listing(
                    &path,
                    dux_core::base_branch::BASE_BRANCH_FETCH_TIMEOUT,
                )
                .map_err(|error| format!("{error:#}"));
                let holders = match &result {
                    Ok(listing) => resolve_branch_holders(&listing.branches, &agents),
                    Err(_) => HashMap::new(),
                };
                (result, holders)
            })
            .unwrap_or_else(|_| {
                (
                    Err("the branch listing worker panicked".to_string()),
                    HashMap::new(),
                )
            });
            let (result, holders) = answer;
            let _ = tx.send(BranchListingAnswer {
                project_id,
                result,
                holders,
            });
        });
        self.pending_branch_listing = Some(PendingBranchListing { rx, op });
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
    }

    /// Fold a landed branch listing into the picker that asked for it, and end
    /// the listing's busy. An answer for a picker the user already closed only
    /// ends the busy.
    pub(crate) fn drain_branch_listing(&mut self) {
        let Some(pending) = self.pending_branch_listing.as_ref() else {
            return;
        };
        let answer = match pending.rx.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let Some(pending) = self.pending_branch_listing.take() else {
            return;
        };
        self.mark_frame_dirty();
        let (project_id, result, holders) = match answer {
            Some(answer) => (Some(answer.project_id), answer.result, answer.holders),
            None => (
                None,
                Err("the branch listing worker stopped without answering".to_string()),
                HashMap::new(),
            ),
        };
        let outcome = match &mut self.prompt {
            PromptState::ChangeBaseBranch(prompt)
                if prompt.loading
                    && project_id
                        .as_deref()
                        .is_none_or(|id| id == prompt.project_id) =>
            {
                prompt.loading = false;
                match result {
                    Ok(listing) => {
                        prompt.fetch_note = listing.fetch_error();
                        prompt.branches = listing.branches;
                        prompt.holders = holders;
                        // `selected` indexes the VISIBLE rows: a search typed
                        // while the list was loading already narrows them.
                        let visible = prompt
                            .list
                            .visible_indices(&prompt.branches, branch_choice_matches);
                        prompt.list.selected = prompt
                            .current_base
                            .as_deref()
                            .and_then(|base| {
                                visible
                                    .iter()
                                    .position(|index| prompt.branches[*index].name == base)
                            })
                            .unwrap_or(0);
                        BranchListingOutcome::Loaded
                    }
                    Err(error) => {
                        prompt.error = Some(error.clone());
                        BranchListingOutcome::Failed(error)
                    }
                }
            }
            _ => BranchListingOutcome::Dismissed,
        };
        self.apply_reaction(pending.op.resolve(&outcome).into_reaction());
    }

    /// Pick the highlighted branch: a free one raises the confirmation, a held
    /// one says why it cannot be picked.
    pub(crate) fn pick_change_base_branch_row(&mut self) {
        let PromptState::ChangeBaseBranch(prompt) = &self.prompt else {
            return;
        };
        if prompt.loading {
            return;
        }
        let visible = prompt
            .list
            .visible_indices(&prompt.branches, branch_choice_matches);
        let Some(branch) = visible
            .get(prompt.list.selected)
            .and_then(|index| prompt.branches.get(*index))
            .cloned()
        else {
            return;
        };
        if let Some(holder) = &branch.held_by {
            let by = match prompt.holders.get(&branch.name) {
                Some(BranchHolder {
                    agent: Some(agent),
                    path,
                }) => format!("agent \"{agent}\" at {path}"),
                Some(BranchHolder { agent: None, path }) => path.clone(),
                None => holder.display().to_string(),
            };
            let message = format!(
                "Can't change the base branch of project \"{}\" to \"{}\": it is checked out \
                 by {by}, and git checks a branch out in one place at a time. Pick another \
                 branch, or remove that worktree first.",
                prompt.project_name, branch.name,
            );
            self.set_warning(message);
            return;
        }
        let previous = (**prompt).clone();
        self.prompt =
            PromptState::ConfirmChangeBaseBranch(Box::new(ConfirmChangeBaseBranchPrompt {
                previous,
                branch: branch.name,
                focus: ConfirmFocus::Cancel, // Cancel is the safe default
            }));
    }

    /// Escape on the picker: leave its search first, then step back to the
    /// action list it came from, or close.
    fn leave_change_base_branch(&mut self) {
        let PromptState::ChangeBaseBranch(prompt) = &mut self.prompt else {
            return;
        };
        if prompt.list.exit_search_clearing_filter() {
            return;
        }
        let return_to = prompt.return_to.take();
        self.prompt = PromptState::None;
        self.return_to_project_actions(return_to);
    }

    /// An outside click on the picker: the same place Escape ends up, in one
    /// step, since a click is not a way to clear a search.
    pub(super) fn dismiss_change_base_branch(&mut self) {
        let PromptState::ChangeBaseBranch(prompt) = &mut self.prompt else {
            return;
        };
        let return_to = prompt.return_to.take();
        self.prompt = PromptState::None;
        self.return_to_project_actions(return_to);
    }

    pub(super) fn handle_change_base_branch_prompt_key(&mut self, key: KeyEvent) -> Option<bool> {
        let PromptState::ChangeBaseBranch(prompt) = &self.prompt else {
            return None;
        };
        let searching = prompt.list.searching;
        let action = if binding_lookup_is_suppressed(key, searching) {
            None
        } else {
            self.bindings
                .lookup(&key, BindingScope::ProjectChooser)
                .or_else(|| self.bindings.lookup(&key, BindingScope::Palette))
                .or_else(|| self.bindings.lookup(&key, BindingScope::Dialog))
        };
        match action {
            Some(Action::CloseOverlay) => self.leave_change_base_branch(),
            // Enter picks the highlighted visible row, mid-search too, as the
            // project list does.
            Some(Action::Confirm) => self.pick_change_base_branch_row(),
            _ => {
                let PromptState::ChangeBaseBranch(prompt) = &mut self.prompt else {
                    return Some(false);
                };
                let visible_len = prompt
                    .list
                    .visible_indices(&prompt.branches, branch_choice_matches)
                    .len();
                match action {
                    Some(Action::SearchToggle) if !searching => prompt.list.begin_search(),
                    Some(Action::MoveDown) => prompt.list.move_down(visible_len),
                    Some(Action::MoveUp) => prompt.list.move_up(),
                    _ if searching && prompt.list.filter.handle_key(key) => {
                        let new_len = prompt
                            .list
                            .visible_indices(&prompt.branches, branch_choice_matches)
                            .len();
                        prompt.list.clamp_selected(new_len);
                    }
                    _ => {}
                }
            }
        }
        Some(false)
    }

    /// A click on the picker's `index`th visible row selects it; a double
    /// click picks it.
    pub(super) fn click_change_base_branch_row(&mut self, index: usize, double_click: bool) {
        if let PromptState::ChangeBaseBranch(prompt) = &mut self.prompt {
            let visible = prompt
                .list
                .visible_indices(&prompt.branches, branch_choice_matches)
                .len();
            if index < visible {
                prompt.list.selected = index;
            }
        }
        if double_click {
            self.pick_change_base_branch_row();
        }
    }

    /// Answer the "change base branch?" confirmation. Confirming hands the
    /// change to the engine (which takes the folder's lock, switches it in a
    /// worker and saves the base); cancelling steps back to the picker and
    /// says nothing moved.
    pub(crate) fn resolve_confirm_change_base_branch(&mut self, confirm: bool) -> bool {
        let PromptState::ConfirmChangeBaseBranch(prompt) = &self.prompt else {
            return false;
        };
        let prompt = (**prompt).clone();
        if !confirm {
            let name = prompt.previous.project_name.clone();
            let Some(project) = self
                .engine
                .projects
                .iter()
                .find(|p| p.id == prompt.previous.project_id)
            else {
                self.prompt = PromptState::None;
                self.set_info(format!(
                    "Project \"{name}\" is gone, so there is no base branch to change."
                ));
                return false;
            };
            let base = project.leading_branch.clone();
            self.prompt = PromptState::ChangeBaseBranch(Box::new(prompt.previous));
            self.set_info(match base {
                Some(base) => format!(
                    "Cancelled changing the base branch of project \"{name}\". Nothing was \
                     checked out, and new worktrees still branch from \"{base}\"."
                ),
                None => format!(
                    "Cancelled changing the base branch of project \"{name}\". Nothing was \
                     checked out, and the project's base branch is unchanged."
                ),
            });
            return false;
        }
        self.prompt = PromptState::None;
        match self
            .engine
            .change_project_base_branch(&prompt.previous.project_id, &prompt.branch)
        {
            Ok(status) => self.apply_reaction(dux_core::engine::EventReaction::Status(status)),
            Err(error) => self.set_error(format!("{error:#}")),
        }
        false
    }

    pub(super) fn handle_confirm_change_base_branch_prompt_key(
        &mut self,
        key: KeyEvent,
    ) -> Option<bool> {
        let PromptState::ConfirmChangeBaseBranch(prompt) = &mut self.prompt else {
            return None;
        };
        let confirm = prompt.focus.is_confirm();
        let action = self.bindings.lookup(&key, BindingScope::Dialog);
        match modal_key_step(action, key, false) {
            ModalKeyStep::Close => return Some(self.resolve_confirm_change_base_branch(false)),
            ModalKeyStep::MoveFocus(_) => prompt.focus = prompt.focus.toggled(),
            ModalKeyStep::Confirm | ModalKeyStep::ActivateFocus => {
                return Some(self.resolve_confirm_change_base_branch(confirm));
            }
            ModalKeyStep::FallThroughToField => {}
        }
        Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen_at(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The Project info screen fits an 80x24 terminal whole: every fact the
    /// browser's dialog shows is on screen, and its Close button is published
    /// inside the screen for the mouse.
    #[test]
    fn project_info_fits_an_80x24_terminal_with_every_fact_and_its_close_button() {
        let mut app = test_app(default_bindings());
        let project = app.engine.projects[0].clone();
        app.open_project_info(&project, None);
        let text = screen_at(&mut app, 80, 24);
        for row in app.project_info_rows(&project) {
            assert!(
                text.contains(&format!("{}:", row.label)),
                "{} is on screen:\n{text}",
                row.label
            );
        }
        match app.overlay_layout.active {
            OverlayMouseLayout::ProjectInfo { close_button } => {
                assert!(
                    close_button.y + close_button.height <= 24,
                    "{close_button:?}"
                );
                assert!(close_button.width > 0);
            }
            other => panic!("expected the info layout, got {other:?}"),
        }
        // Space closes it too; with nothing behind it the stack closes.
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(app.prompt, PromptState::None));
    }

    /// A project's name and branches are chips in the action list's header,
    /// and a project with no base says so in words.
    #[test]
    fn the_action_list_header_names_the_project_its_folder_and_both_branches() {
        let mut app = test_app(default_bindings());
        let mut project = app.engine.projects[0].clone();
        project.leading_branch = None;
        project.current_branch = "topic".to_string();
        app.engine.projects[0] = project.clone();
        app.open_project_actions(
            ProjectActionsTarget::Project {
                id: project.id.clone(),
            },
            None,
        );
        let text = screen_at(&mut app, 80, 24);
        assert!(
            text.contains(&format!("Project:  {} ", project.name)),
            "{text}"
        );
        assert!(text.contains("Base branch: no base recorded yet"), "{text}");
        assert!(text.contains("Folder is on:  topic "), "{text}");
        assert!(text.contains("Folder: "), "{text}");
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
            .unwrap();
    }

    fn branch(name: &str) -> BranchChoice {
        BranchChoice {
            name: name.to_string(),
            location: dux_core::git::BranchLocation::Local,
            held_by: None,
        }
    }

    fn picker(app: &App, loading: bool, branches: Vec<BranchChoice>) -> PromptState {
        let project = &app.engine.projects[0];
        PromptState::ChangeBaseBranch(Box::new(ChangeBaseBranchPrompt {
            holders: std::collections::HashMap::new(),
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            current_base: Some("develop".to_string()),
            loading,
            branches,
            fetch_note: None,
            error: None,
            list: SearchableList::new(),
            return_to: None,
        }))
    }

    fn type_search(app: &mut App, query: &str) {
        press(app, KeyCode::Char('/'));
        for c in query.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    /// Enter in the picker's search row picks the highlighted visible branch,
    /// as the project list does, so the footer's "choose" is true.
    #[test]
    fn enter_while_searching_the_branches_picks_the_highlighted_one() {
        let mut app = test_app(default_bindings());
        app.prompt = picker(
            &app,
            false,
            vec![branch("develop"), branch("main"), branch("release")],
        );
        type_search(&mut app, "rel");
        press(&mut app, KeyCode::Enter);
        match &app.prompt {
            PromptState::ConfirmChangeBaseBranch(confirm) => assert_eq!(confirm.branch, "release"),
            other => panic!("one Enter picks the match, got {other:?}"),
        }
    }

    /// A listing that lands while a search is narrowing the rows selects
    /// within the visible rows, so Enter picks what is highlighted.
    #[test]
    fn a_listing_that_lands_mid_search_selects_among_the_visible_rows() {
        let mut app = test_app(default_bindings());
        app.prompt = picker(&app, true, Vec::new());
        type_search(&mut app, "rel");
        press(&mut app, KeyCode::Enter);
        let project_id = app.engine.projects[0].id.clone();
        let (tx, rx) = mpsc::channel();
        let op = dux_core::engine::status_op("Listing...")
            .resolve_in_handler(|_: &BranchListingOutcome| dux_core::engine::Final::clear());
        app.pending_branch_listing = Some(PendingBranchListing { rx, op });
        tx.send(BranchListingAnswer {
            holders: std::collections::HashMap::new(),
            project_id,
            result: Ok(dux_core::base_branch::BranchListing {
                branches: vec![
                    branch("alpha"),
                    branch("beta"),
                    branch("develop"),
                    branch("release"),
                ],
                fetch: dux_core::base_branch::OriginFetch::Fetched,
            }),
        })
        .unwrap();
        app.drain_branch_listing();

        press(&mut app, KeyCode::Enter);
        match &app.prompt {
            PromptState::ConfirmChangeBaseBranch(confirm) => assert_eq!(confirm.branch, "release"),
            other => panic!("Enter picks the one visible row, got {other:?}"),
        }
    }

    /// The header's folder is a name, so it is the chip.
    #[test]
    fn the_action_list_header_chips_the_folder() {
        let mut app = test_app(default_bindings());
        let project = app.engine.projects[0].clone();
        app.open_project_actions(
            ProjectActionsTarget::Project {
                id: project.id.clone(),
            },
            None,
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buffer = terminal.backend().buffer().clone();
        let chip_bg = app.theme.name_style().bg;
        let row = (0..buffer.area.height)
            .find(|&y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("Folder: ")
            })
            .expect("the folder row");
        let line: String = (0..buffer.area.width)
            .map(|x| buffer[(x, row)].symbol())
            .collect();
        let start = line.find("Folder: ").unwrap() + "Folder: ".len();
        let x = line[..start].chars().count() as u16 + 1;
        assert_eq!(
            buffer[(x, row)].bg,
            chip_bg.expect("chip bg"),
            "the folder path is chipped: {line}"
        );
    }

    /// When the rows change under the cursor (GitHub becomes unavailable and
    /// its row leaves), the cursor stays on the action it was on and Enter
    /// runs that action, not whatever now sits at the old index.
    #[test]
    fn the_action_list_follows_its_action_when_the_rows_change() {
        let mut app = test_app(default_bindings());
        app.engine.github_integration_enabled = true;
        app.engine.gh_status = crate::model::GhStatus::Available;
        let project = app.engine.projects[0].clone();
        app.open_project_actions(
            ProjectActionsTarget::Project {
                id: project.id.clone(),
            },
            None,
        );
        let rows = app
            .project_action_rows(&ProjectActionsTarget::Project {
                id: project.id.clone(),
            })
            .len();
        for _ in 0..rows {
            press(&mut app, KeyCode::Down);
        }
        // The last row is Remove project…; GitHub goes away under it.
        app.engine.gh_status = crate::model::GhStatus::NotInstalled;
        press(&mut app, KeyCode::Enter);
        match &app.prompt {
            PromptState::ConfirmRemoveProject { .. } => {}
            // The seeded project has an agent, so removal is refused out loud,
            // which is still the Remove action running.
            PromptState::ProjectActions(_) => assert_eq!(
                app.status.text(),
                "Delete all agents in this project first."
            ),
            other => panic!("Enter runs Remove project…, got {other:?}"),
        }
    }

    /// A held row's holder, agent and folder, are names, so both are chips.
    #[test]
    fn a_held_branch_row_chips_its_agent_and_folder() {
        let mut app = test_app(default_bindings());
        let mut held = branch("agent-one");
        held.held_by = Some(PathBuf::from("/srv/wt/agent-one"));
        app.prompt = picker(&app, false, vec![branch("develop"), held]);
        if let PromptState::ChangeBaseBranch(prompt) = &mut app.prompt {
            prompt.holders.insert(
                "agent-one".to_string(),
                BranchHolder {
                    agent: Some("Alpha".to_string()),
                    path: "/srv/wt/agent-one".to_string(),
                },
            );
        }
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buffer = terminal.backend().buffer().clone();
        let chip_bg = app.theme.name_style().bg.expect("chip bg");
        let chipped = |needle: &str| {
            (0..buffer.area.height).any(|y| {
                let line: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                line.find(needle).is_some_and(|at| {
                    let x = line[..at].chars().count() as u16;
                    buffer[(x, y)].bg == chip_bg
                })
            })
        };
        assert!(chipped("Alpha"), "the agent is a chip");
        assert!(chipped("/srv/wt/agent-one"), "the folder is a chip");
    }
}
