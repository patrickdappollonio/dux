//! The TUI worktree manager: the manual override for removing a worktree, and
//! the branch with it.
//!
//! Its checkbox is honored whatever the branch's origin (see
//! [`dux_core::model::BranchProvenance`]), because the user is pointing at one
//! specific worktree. Which worktrees the manager owns, which are removable and
//! what a removal does to the branch live in [`dux_core::worktree_manager`],
//! shared with the web route; what lives here is opening the picker and the
//! confirmation and dispatching the removal onto a background worker.
//!
//! It also lists the folders a removal left behind (see
//! [`dux_core::leftover_folders`]), each with its own action list: copy the
//! path, or delete the folder behind a confirmation.

use super::{
    App, ConfirmAnswer, ConfirmDeleteLeftoverFolderPrompt, ConfirmDeleteWorktreePrompt,
    DeleteWorktreeFocus, FullscreenOverlay, InputTarget, LeftoverFolderAction,
    LeftoverFolderActionsPrompt, ManageWorktreesPrompt, ManagerSelection, ProjectActionsPrompt,
    ProjectChooserIntent, PromptState, WorktreesFinalOutcome,
};
use crate::model::Project;

impl App {
    /// `manage-worktrees`: open the project chooser, then (per project) load
    /// that project's manageable worktrees into [`PromptState::ManageWorktrees`].
    pub(crate) fn manage_project_worktrees(&mut self) {
        {
            self.open_project_chooser(ProjectChooserIntent::ManageWorktrees);
        }
    }

    /// Per-project body for `ManageWorktrees`: opens the manager and spawns the
    /// listing worker.
    pub(crate) fn begin_manage_worktrees_for_project(
        &mut self,
        project: Project,
        return_to: Option<Box<ProjectActionsPrompt>>,
    ) {
        if project.path_missing {
            self.prompt = PromptState::None;
            self.set_warning(format!("Project path not found: {}", project.path));
            return;
        }

        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ManageWorktrees(ManageWorktreesPrompt {
            return_to,
            project: project.clone(),
            entries: Vec::new(),
            leftovers: Vec::new(),
            loading: true,
            selected: None,
            error: None,
        });
        // Declare the loading→final states together, the same way the adopt
        // picker does: the final depends on whether the manager is still open
        // and matching when the listing arrives, which the worker cannot see.
        let project_name = project.name.clone();
        let op = dux_core::engine::status_op(
            "Listing the worktrees dux manages for the selected project...",
        )
        .resolve_in_handler(move |o: &WorktreesFinalOutcome| match o {
            WorktreesFinalOutcome::Loaded => dux_core::engine::Final::info(
                "Choose a worktree to remove. Worktrees an agent holds are listed but cannot be \
                 removed here; delete that agent first. Deleting a standalone agent leaves its \
                 directory in place, so remove that one yourself.",
            ),
            WorktreesFinalOutcome::Failed(error) => dux_core::engine::Final::error(format!(
                "Failed to list worktrees for project \"{project_name}\": {error}"
            )),
            WorktreesFinalOutcome::Dismissed => dux_core::engine::Final::clear(),
        });
        let pending = self.engine.begin_status_op(&op);
        let op_id = op.id().to_string();
        self.pending_worktree_ops.insert(op_id.clone(), op);
        self.engine
            .spawn_manageable_worktrees_worker(project, Some(op_id));
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
    }

    /// Act on the manager's selected row: a worktree raises its removal
    /// confirmation, a left-over folder opens its actions.
    pub(crate) fn activate_manager_selection(&mut self) {
        let PromptState::ManageWorktrees(prompt) = &self.prompt else {
            return;
        };
        match prompt.selected {
            Some(ManagerSelection::Leftover(index)) => self.open_leftover_folder_actions(index),
            Some(ManagerSelection::Worktree(_)) | None => self.confirm_delete_selected_worktree(),
        }
    }

    /// Open the manager's `index`th left-over folder's actions.
    fn open_leftover_folder_actions(&mut self, index: usize) {
        let PromptState::ManageWorktrees(prompt) = &self.prompt else {
            return;
        };
        let Some(folder) = prompt.leftovers.get(index).cloned() else {
            self.set_error("No left-over folder is selected.");
            return;
        };
        self.prompt = PromptState::LeftoverFolderActions(Box::new(
            LeftoverFolderActionsPrompt::new(prompt.clone(), folder),
        ));
    }

    /// A fresh listing of `project_id`'s manager landing while a left-over
    /// folder's actions or its delete confirmation is open over it. Both act
    /// on the row they were opened from, so they stay open only while the
    /// fresh row for the same folder is that row exactly (its branches, the
    /// ones a delete would keep, whether the folder is there, what is using
    /// it); the manager behind them takes the fresh listing either way.
    /// Otherwise they close onto the refreshed manager, and the returned
    /// sentence says why. `None` when neither is open over this project's
    /// manager, or when they stay open.
    pub(super) fn refresh_open_leftover_prompt(
        &mut self,
        project_id: &str,
        entries: Vec<dux_core::worktree_manager::ManagedWorktree>,
        leftovers: Vec<dux_core::leftover_folders::LeftoverFolder>,
    ) -> Option<String> {
        let (manager, folder) = match &mut self.prompt {
            PromptState::LeftoverFolderActions(prompt) => (&mut prompt.previous, &prompt.folder),
            PromptState::ConfirmDeleteLeftoverFolder(prompt) => {
                (&mut prompt.previous.previous, &prompt.previous.folder)
            }
            _ => return None,
        };
        if manager.project.id != project_id {
            return None;
        }
        let fresh = leftovers.iter().position(|row| row.path == folder.path);
        let unchanged = fresh.is_some_and(|index| leftovers[index] == *folder);
        let note = match fresh {
            _ if unchanged => None,
            Some(_) => Some(format!(
                "The row for the left-over folder at {} changed while it was open, so it was \
                 closed before anything was deleted. Look again at the refreshed worktree \
                 manager before deleting it.",
                folder.path.display()
            )),
            None => Some(format!(
                "The left-over folder at {} is no longer listed, so nothing was deleted. Look \
                 again at the refreshed worktree manager.",
                folder.path.display()
            )),
        };
        manager.selected = fresh.map(ManagerSelection::Leftover).or_else(|| {
            crate::app::manager_selectable_rows(&entries, &leftovers)
                .into_iter()
                .next()
        });
        manager.entries = entries;
        manager.leftovers = leftovers;
        manager.loading = false;
        manager.error = None;
        if note.is_some() {
            let manager = manager.clone();
            self.prompt = PromptState::ManageWorktrees(manager);
        }
        note
    }

    /// Close a left-over folder's actions, back onto the manager.
    pub(super) fn close_leftover_folder_actions(&mut self) {
        let PromptState::LeftoverFolderActions(prompt) = &self.prompt else {
            return;
        };
        self.prompt = PromptState::ManageWorktrees(prompt.previous.clone());
    }

    /// Run the action under the left-over folder list's cursor.
    pub(super) fn activate_leftover_folder_action(&mut self) {
        let PromptState::LeftoverFolderActions(prompt) = &self.prompt else {
            return;
        };
        let prompt = (**prompt).clone();
        match prompt.actions.get(prompt.selected).copied() {
            Some(LeftoverFolderAction::CopyPath) => {
                self.prompt = PromptState::ManageWorktrees(prompt.previous.clone());
                self.copy_to_clipboard(&prompt.folder.path.to_string_lossy(), "folder path");
            }
            Some(LeftoverFolderAction::DeleteFolder) => {
                let delete_branches =
                    !prompt.folder.folder_present || prompt.folder.deletes_branches_by_default();
                self.prompt = PromptState::ConfirmDeleteLeftoverFolder(Box::new(
                    ConfirmDeleteLeftoverFolderPrompt {
                        previous: prompt,
                        delete_branches,
                        focus: DeleteWorktreeFocus::Cancel,
                    },
                ));
            }
            None => {}
        }
    }

    /// Resolve the left-over folder delete's confirmation. Cancelling returns
    /// to the folder's actions (nothing was touched); confirming closes every
    /// overlay and dispatches the delete, for the reason
    /// [`Self::resolve_confirm_delete_worktree`] gives.
    pub(super) fn resolve_confirm_delete_leftover_folder(&mut self, answer: ConfirmAnswer) -> bool {
        let PromptState::ConfirmDeleteLeftoverFolder(prompt) = &self.prompt else {
            return false;
        };
        let prompt = (**prompt).clone();
        if answer == ConfirmAnswer::Cancel {
            self.prompt = PromptState::LeftoverFolderActions(Box::new(prompt.previous));
            self.set_info("Delete cancelled. The left-over folder and its branches are unchanged.");
            return false;
        }
        self.prompt = PromptState::None;
        self.dispatch_leftover_folder_removal(&prompt);
        self.return_to_project_actions(prompt.previous.previous.return_to.clone());
        false
    }

    /// Delete the folder on a background worker, with a keyed Busy and the
    /// shared report of what happened as the final.
    fn dispatch_leftover_folder_removal(&mut self, prompt: &ConfirmDeleteLeftoverFolderPrompt) {
        let folder = prompt.folder();
        let path = folder.path.clone();
        let display_path = path.to_string_lossy().to_string();
        let delete_branches = if prompt.delete_branches {
            folder.branches.clone()
        } else {
            Vec::new()
        };
        let admission = self.engine.admit_leftover_removal(
            &prompt.previous.previous.project.id,
            &path,
            delete_branches,
        );
        let report_path = display_path.clone();
        let outcome_final = move |outcome: &dux_core::leftover_folders::LeftoverRemovalOutcome| {
            leftover_removal_final(&report_path, outcome)
        };
        let ticket = match admission {
            None => {
                self.set_error(format!(
                    "Nothing was deleted: the project that owned {display_path} is no longer in dux."
                ));
                return;
            }
            Some(dux_core::leftover_folders::LeftoverAdmission::Refused(outcome)) => {
                let resolved = dux_core::engine::ResolvedFinal::new(
                    "leftover-folder-removal-refused",
                    outcome_final(&outcome),
                );
                self.apply_reaction(resolved.into_reaction());
                return;
            }
            Some(dux_core::leftover_folders::LeftoverAdmission::Admitted(ticket)) => ticket,
        };
        let op = dux_core::engine::status_op(dux_core::status_text![
            "Deleting the left-over folder at ",
            n(display_path),
            "..."
        ])
        .on_success(outcome_final)
        .on_failure(move |error: &String| {
            dux_core::engine::Final::error(dux_core::status_text![
                "Could not delete the left-over folder at ",
                n(display_path),
                ": ",
                error.as_str()
            ])
        });
        let reaction = self.engine.spawn_status_op(op, move || ticket.run());
        self.apply_reaction(reaction);
    }

    /// Open the removal confirmation for the manager's selected row.
    ///
    /// A row an agent holds is refused with the same sentence the web route's
    /// 409 carries; the worktree stays listed, since hiding it would leave the
    /// user hunting for a directory they can see.
    pub(crate) fn confirm_delete_selected_worktree(&mut self) {
        let PromptState::ManageWorktrees(prompt) = &self.prompt else {
            return;
        };
        let Some(entry) = (match prompt.selected {
            Some(ManagerSelection::Worktree(index)) => prompt.entries.get(index),
            Some(ManagerSelection::Leftover(_)) | None => None,
        }) else {
            self.set_error("No worktree is selected.");
            return;
        };
        if !entry.is_removable() {
            self.set_error(
                "An agent is working in that directory; delete that agent first. If it is a \
                 standalone agent, deleting it leaves the directory in place, so remove that one \
                 yourself.",
            );
            return;
        }
        let confirm = ConfirmDeleteWorktreePrompt {
            previous: prompt.clone(),
            project: prompt.project.clone(),
            path: entry.path.clone(),
            label: entry.label.clone(),
            branch: entry.branch.clone(),
            dirty: entry.dirty,
            // Default ON, matching the web dialog's checkbox: this surface
            // exists to delete a branch by hand, so the common case is the
            // default and the user unticks to keep it.
            delete_branch: true,
            focus: DeleteWorktreeFocus::Cancel,
        };
        self.prompt = PromptState::ConfirmDeleteWorktree(Box::new(confirm));
    }

    /// Resolve the removal confirmation. Cancelling returns to the manager
    /// (nothing was touched); confirming closes every overlay and dispatches
    /// the removal.
    ///
    /// Confirming deliberately does not put the manager back: relisting opens a
    /// second keyed Busy, and on a single most-recent-wins status line the
    /// listing's chatter buries the removal's own final report.
    pub(super) fn resolve_confirm_delete_worktree(&mut self, answer: ConfirmAnswer) -> bool {
        let PromptState::ConfirmDeleteWorktree(prompt) = &self.prompt else {
            return false;
        };
        let prompt = prompt.clone();
        if answer == ConfirmAnswer::Cancel {
            self.prompt = PromptState::ManageWorktrees(prompt.previous);
            self.set_info("Removal cancelled. The worktree and its branch are unchanged.");
            return false;
        }
        self.prompt = PromptState::None;
        self.dispatch_worktree_removal(&prompt);
        // A manager opened from a project's action list closes back onto it.
        self.return_to_project_actions(prompt.previous.return_to.clone());
        false
    }

    /// Run the removal on a background worker, with a keyed Busy and a verbose
    /// final that says what actually happened to the branch (never what the
    /// checkbox asked for: `git branch -D` can refuse).
    fn dispatch_worktree_removal(&mut self, prompt: &ConfirmDeleteWorktreePrompt) {
        let path = prompt.path.clone();
        let delete_branch = prompt.delete_branch;
        let display_path = path.to_string_lossy().to_string();
        let failure_path = display_path.clone();
        // Decided by the engine against LIVE state at this moment (the agents
        // now, an agent being created now, a removal already under way), never
        // against the listing on screen, and announced before it returns.
        let admission = self.engine.admit_manager_removal(
            &prompt.project.id,
            &path,
            if delete_branch {
                dux_core::worktree_manager::BranchRemoval::Delete
            } else {
                dux_core::worktree_manager::BranchRemoval::Keep
            },
        );
        let outcome_final = move |outcome: &dux_core::worktree_manager::RemovalOutcome| {
            worktree_removal_final(&display_path, outcome)
        };
        let ticket = match admission {
            None => {
                self.set_error(format!(
                    "Nothing was removed: the project that owned {failure_path} is no longer in dux."
                ));
                return;
            }
            Some(dux_core::worktree_manager::RemovalAdmission::Refused(outcome)) => {
                let resolved = dux_core::engine::ResolvedFinal::new(
                    "worktree-removal-refused",
                    outcome_final(&outcome),
                );
                self.apply_reaction(resolved.into_reaction());
                return;
            }
            Some(dux_core::worktree_manager::RemovalAdmission::Admitted(ticket)) => ticket,
        };
        let busy = match ticket.waiting_for() {
            Some(waiting) => format!(
                "Removing the worktree at {failure_path}: waiting for {waiting} in it to finish first..."
            ),
            None => format!("Removing the worktree at {failure_path}..."),
        };
        let op = dux_core::engine::status_op(busy)
            .on_success(outcome_final)
            .on_failure(move |error: &String| {
                dux_core::engine::Final::error(format!(
                    "Could not remove the worktree at {failure_path}: {error}"
                ))
            });
        let reaction = self.engine.spawn_status_op(op, move || ticket.run());
        self.apply_reaction(reaction);
    }
}

/// The final for a manager removal, said from what actually happened (never
/// what the checkbox asked for: `git branch -D` can refuse). The refusals are
/// decided against live state at removal time, so they can differ from what
/// the manager showed a moment ago; each says which one happened.
fn worktree_removal_final(
    display_path: &str,
    outcome: &dux_core::worktree_manager::RemovalOutcome,
) -> dux_core::engine::Final {
    match outcome {
        dux_core::worktree_manager::RemovalOutcome::NotManaged => {
            dux_core::engine::Final::warning(format!(
                "Nothing was removed: {display_path} is no longer a worktree dux manages \
                 for this project. Reopen the worktree manager to see the current list."
            ))
        }
        dux_core::worktree_manager::RemovalOutcome::Attached => {
            dux_core::engine::Final::warning(format!(
                "Nothing was removed: an agent is attached to {display_path}, or one is being \
                 created on it. Delete that agent first. Deleting a managed agent removes its \
                 worktree with it; deleting a standalone agent leaves its directory in place, \
                 so remove that one yourself."
            ))
        }
        dux_core::worktree_manager::RemovalOutcome::Busy { reason } => {
            dux_core::engine::Final::warning(format!(
                "Nothing was removed: {reason} in {display_path}. Try again once it has stopped \
                 (close that terminal, or wait for the deleted agent to finish stopping)."
            ))
        }
        dux_core::worktree_manager::RemovalOutcome::BeingRemoved => {
            dux_core::engine::Final::warning(format!(
                "Nothing more was done: {display_path} is already being removed, because the \
                 agent that owned it was just deleted. It disappears from the manager once \
                 that removal finishes."
            ))
        }
        dux_core::worktree_manager::RemovalOutcome::Removed { path, branch } => {
            let report = dux_core::worktree_manager::removal_report(
                path.to_string_lossy().as_ref(),
                branch.as_ref(),
            );
            if report.warning {
                dux_core::engine::Final::warning(report.message)
            } else {
                dux_core::engine::Final::info(report.message)
            }
        }
    }
}

/// The final for a left-over folder delete: the sentence both surfaces share.
fn leftover_removal_final(
    display_path: &str,
    outcome: &dux_core::leftover_folders::LeftoverRemovalOutcome,
) -> dux_core::engine::Final {
    let report = dux_core::leftover_folders::leftover_removal_report(display_path, outcome);
    if report.warning {
        dux_core::engine::Final::warning(report.message)
    } else {
        dux_core::engine::Final::info(report.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use crate::app::{
        Listing, ManageWorktreeVisualRow, manage_worktree_visual_rows, removable_worktree_indices,
    };
    use dux_core::worktree_manager::ManagedWorktree;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    /// The manager as a screen of text, with `entries` listed.
    fn manager_screen(entries: Vec<ManagedWorktree>) -> String {
        let mut app = test_app(default_bindings());
        let project = app.engine.projects[0].clone();
        app.prompt = PromptState::ManageWorktrees(ManageWorktreesPrompt {
            return_to: None,
            project,
            entries,
            leftovers: Vec::new(),
            loading: false,
            selected: None,
            error: None,
        });
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 40)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn held_row(name: &str, attached: Option<&str>, busy: Option<&str>) -> ManagedWorktree {
        ManagedWorktree {
            path: PathBuf::from(format!("/srv/worktrees/demo/{name}")),
            label: format!("br-{name}"),
            branch: Some(format!("br-{name}")),
            dirty: false,
            attached_session_id: attached.map(str::to_string),
            being_removed: false,
            busy: busy.map(str::to_string),
        }
    }

    /// A worktree its own agent holds reads "held by an agent", under that
    /// header, and no "in use" line: the agent that owns a worktree is not
    /// something else using it.
    #[test]
    fn an_agent_held_row_reads_held_by_an_agent_and_not_in_use() {
        let screen = manager_screen(vec![held_row("held", Some("s1"), None)]);
        let line = screen
            .lines()
            .find(|line| line.contains("br-held"))
            .expect("the row is listed");
        assert!(line.contains("held by an agent"), "{line}");
        assert!(!line.contains("in use"), "{line}");
        assert!(screen.contains("Held By An Agent"), "{screen}");
    }

    /// Held by its agent AND used by something else: both are said.
    #[test]
    fn an_agent_held_row_something_else_uses_says_both() {
        let screen = manager_screen(vec![held_row(
            "both",
            Some("s1"),
            Some("a terminal open in it"),
        )]);
        let line = screen
            .lines()
            .find(|line| line.contains("br-both"))
            .expect("the row is listed");
        assert!(line.contains("held by an agent"), "{line}");
        assert!(line.contains("in use: a terminal open in it"), "{line}");
    }

    /// No agent holds it, something uses it: "in use", as before.
    #[test]
    fn a_row_no_agent_holds_but_something_uses_reads_in_use() {
        let screen = manager_screen(vec![held_row("busy", None, Some("a terminal open in it"))]);
        let line = screen
            .lines()
            .find(|line| line.contains("br-busy"))
            .expect("the row is listed");
        assert!(line.contains("in use: a terminal open in it"), "{line}");
        assert!(!line.contains("held by an agent"), "{line}");
    }

    #[test]
    fn the_listing_spinner_survives_the_busy_timeout_and_is_retired_by_its_final() {
        // The TUI keeps several op registries of its own, in the App rather than
        // the engine, and the first cut of liveness could not see any of them:
        // this spinner was replaced by a false "timed out" twenty seconds in.
        // The App registers into the ENGINE's live-key set, which is the same set
        // the App's status controller retires from, so there is one mechanism
        // rather than one per layer.
        let mut app = test_app(default_bindings());
        let project = app.engine.projects[0].clone();
        app.begin_manage_worktrees_for_project(project, None);

        let op_id = app
            .pending_worktree_ops
            .keys()
            .next()
            .cloned()
            .expect("the listing op is registered");
        assert!(app.engine.status_op_is_live(&op_id));

        let t0 = std::time::Instant::now();
        let changes = app.status.tick(
            t0 + dux_core::statusline::BUSY_TIMEOUT * 2,
            dux_core::statusline::BUSY_TIMEOUT,
        );
        assert!(
            changes.upgraded.is_empty(),
            "a listing that is still running must not be reported as timed out"
        );
        assert_eq!(app.status.snapshot()[0].tone, "busy");

        // The op resolves the way the completion handler resolves it.
        let op = app
            .pending_worktree_ops
            .remove(&op_id)
            .expect("the op is still there");
        let resolved = op.resolve(&WorktreesFinalOutcome::Loaded);
        app.apply_reaction(resolved.into_reaction());
        assert!(
            !app.engine.status_op_is_live(&op_id),
            "and its final retires the registration"
        );
    }

    fn row(
        name: &str,
        branch: Option<&str>,
        agent: Option<&str>,
        state: crate::app::test_support::WorktreeState,
    ) -> ManagedWorktree {
        ManagedWorktree {
            path: PathBuf::from(format!("/tmp/worktrees/demo/{name}")),
            label: branch.unwrap_or("detached abc1234").to_string(),
            branch: branch.map(str::to_string),
            dirty: state == crate::app::test_support::WorktreeState::Dirty,
            attached_session_id: agent.map(str::to_string),
            being_removed: false,
            busy: None,
        }
    }

    fn manager(entries: Vec<ManagedWorktree>) -> ManageWorktreesPrompt {
        let mut app = test_app(default_bindings());
        let project = app.engine.projects[0].clone();
        app.prompt = PromptState::None;
        ManageWorktreesPrompt {
            return_to: None,
            project,
            selected: removable_worktree_indices(&entries)
                .into_iter()
                .next()
                .map(ManagerSelection::Worktree),
            entries,
            leftovers: Vec::new(),
            loading: false,
            error: None,
        }
    }

    fn open_manager(app: &mut App, entries: Vec<ManagedWorktree>) {
        let project = app.engine.projects[0].clone();
        let mut prompt = manager(entries);
        prompt.project = project;
        app.prompt = PromptState::ManageWorktrees(prompt);
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn only_free_worktrees_are_selectable_and_held_ones_are_still_listed() {
        let entries = vec![
            row(
                "held",
                Some("held"),
                Some("session-1"),
                crate::app::test_support::WorktreeState::Clean,
            ),
            row(
                "free",
                Some("free"),
                None,
                crate::app::test_support::WorktreeState::Clean,
            ),
        ];
        assert_eq!(removable_worktree_indices(&entries), vec![1]);
        let rows = manage_worktree_visual_rows(&entries, &[], Listing::Loaded, None);
        assert!(
            rows.iter()
                .any(|r| matches!(r, ManageWorktreeVisualRow::Entry(0))),
            "an attached worktree is listed, not hidden: {rows:?}"
        );
    }

    #[test]
    fn picking_a_free_worktree_opens_the_confirmation_with_the_branch_checkbox_on() {
        let mut app = test_app(default_bindings());
        open_manager(
            &mut app,
            vec![row(
                "free",
                Some("free"),
                None,
                crate::app::test_support::WorktreeState::Clean,
            )],
        );
        app.confirm_delete_selected_worktree();
        let PromptState::ConfirmDeleteWorktree(prompt) = &app.prompt else {
            panic!("expected the confirmation, got {:?}", app.prompt);
        };
        assert!(
            prompt.delete_branch,
            "the branch checkbox defaults ON, matching the web dialog"
        );
        assert_eq!(prompt.branch.as_deref(), Some("free"));
        assert_eq!(
            prompt.focus,
            DeleteWorktreeFocus::Cancel,
            "the safe control has focus"
        );
    }

    #[test]
    fn picking_an_attached_worktree_is_refused_rather_than_confirmed() {
        let mut app = test_app(default_bindings());
        let entries = vec![row(
            "held",
            Some("held"),
            Some("session-1"),
            crate::app::test_support::WorktreeState::Clean,
        )];
        let mut prompt = manager(entries);
        prompt.selected = Some(ManagerSelection::Worktree(0));
        app.prompt = PromptState::ManageWorktrees(prompt);
        app.confirm_delete_selected_worktree();
        assert!(
            matches!(app.prompt, PromptState::ManageWorktrees(_)),
            "the manager stays open"
        );
        assert!(
            app.status.message().contains("delete that agent first"),
            "got {:?}",
            app.status.message()
        );
    }

    #[test]
    fn a_detached_worktree_has_no_branch_checkbox() {
        let mut app = test_app(default_bindings());
        open_manager(
            &mut app,
            vec![row(
                "loose",
                None,
                None,
                crate::app::test_support::WorktreeState::Clean,
            )],
        );
        app.confirm_delete_selected_worktree();
        let PromptState::ConfirmDeleteWorktree(prompt) = &app.prompt else {
            panic!("expected the confirmation");
        };
        assert!(prompt.branch.is_none());
        assert!(
            !prompt.has_branch_checkbox(),
            "there is no branch to keep or delete, so no checkbox is offered"
        );
    }

    #[test]
    fn space_toggles_the_focused_checkbox_and_movement_only_moves_focus() {
        let mut app = test_app(default_bindings());
        open_manager(
            &mut app,
            vec![row(
                "free",
                Some("free"),
                None,
                crate::app::test_support::WorktreeState::Clean,
            )],
        );
        app.confirm_delete_selected_worktree();
        // Focus moves Cancel -> Delete -> Checkbox with the horizontal key.
        app.handle_key(key(KeyCode::Right)).unwrap();
        app.handle_key(key(KeyCode::Right)).unwrap();
        let PromptState::ConfirmDeleteWorktree(prompt) = &app.prompt else {
            panic!("expected the confirmation");
        };
        assert_eq!(prompt.focus, DeleteWorktreeFocus::Checkbox);
        assert!(prompt.delete_branch, "moving focus never changes a value");
        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        let PromptState::ConfirmDeleteWorktree(prompt) = &app.prompt else {
            panic!("expected the confirmation");
        };
        assert!(!prompt.delete_branch, "Space toggles the focused checkbox");
    }

    #[test]
    fn the_focus_ring_skips_the_checkbox_when_there_is_no_branch() {
        let mut app = test_app(default_bindings());
        open_manager(
            &mut app,
            vec![row(
                "loose",
                None,
                None,
                crate::app::test_support::WorktreeState::Clean,
            )],
        );
        app.confirm_delete_selected_worktree();
        app.handle_key(key(KeyCode::Right)).unwrap();
        app.handle_key(key(KeyCode::Right)).unwrap();
        let PromptState::ConfirmDeleteWorktree(prompt) = &app.prompt else {
            panic!("expected the confirmation");
        };
        assert_eq!(
            prompt.focus,
            DeleteWorktreeFocus::Cancel,
            "a two-stop ring comes back to Cancel"
        );
    }

    #[test]
    fn escape_abandons_the_confirmation_and_returns_to_the_manager() {
        let mut app = test_app(default_bindings());
        open_manager(
            &mut app,
            vec![row(
                "free",
                Some("free"),
                None,
                crate::app::test_support::WorktreeState::Clean,
            )],
        );
        app.confirm_delete_selected_worktree();
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(
            matches!(app.prompt, PromptState::ManageWorktrees(_)),
            "Esc abandons the removal and puts the list back, got {:?}",
            app.prompt
        );
    }

    #[test]
    fn cancelling_says_nothing_was_touched() {
        let mut app = test_app(default_bindings());
        open_manager(
            &mut app,
            vec![row(
                "free",
                Some("free"),
                None,
                crate::app::test_support::WorktreeState::Clean,
            )],
        );
        app.confirm_delete_selected_worktree();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.prompt, PromptState::ManageWorktrees(_)));
        assert!(
            app.status.message().contains("unchanged"),
            "got {:?}",
            app.status.message()
        );
    }

    /// Confirming closes every overlay rather than putting the manager back,
    /// and that is a decision. See `resolve_confirm_delete_worktree`.
    #[test]
    fn confirming_the_removal_closes_the_overlay_rather_than_relisting() {
        let mut app = test_app(default_bindings());
        open_manager(
            &mut app,
            vec![row(
                "free",
                Some("free"),
                None,
                crate::app::test_support::WorktreeState::Clean,
            )],
        );
        app.confirm_delete_selected_worktree();
        app.handle_key(key(KeyCode::Right)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(
            matches!(app.prompt, PromptState::None),
            "the removal's own status is the report, and a fresh listing's Busy \
             would fight it on the one status line, got {:?}",
            app.prompt
        );
    }

    /// Drive the whole surface against a real repository: the manager's
    /// listing, the confirmation, and the background removal.
    ///
    /// Returns the app, the repo path and the worktree path, with the
    /// confirmation open on the one removable worktree.
    fn app_with_a_real_worktree(
        branch_removal: dux_core::worktree_manager::BranchRemoval,
    ) -> (App, PathBuf, PathBuf) {
        use crate::app::test_support::run_git;
        let mut app = test_app(default_bindings());
        let project = app.engine.projects[0].clone();
        let repo = PathBuf::from(&project.path);
        let worktree = app
            .engine
            .paths
            .worktrees_root
            .join(&project.name)
            .join("free");
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "free",
                worktree.to_string_lossy().as_ref(),
            ],
        );

        // Load the manager the way the palette command does, then wait for the
        // listing worker (a real dependency, bounded).
        app.begin_manage_worktrees_for_project(project, None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.drain_events();
            let loaded = matches!(&app.prompt, PromptState::ManageWorktrees(p) if !p.loading);
            if loaded || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let PromptState::ManageWorktrees(prompt) = &app.prompt else {
            panic!("the manager must still be open, got {:?}", app.prompt);
        };
        assert!(!prompt.loading, "the listing must have arrived");
        assert_eq!(
            prompt.entries.len(),
            1,
            "only the managed worktree is listed: {:?}",
            prompt.entries
        );

        app.confirm_delete_selected_worktree();
        if let PromptState::ConfirmDeleteWorktree(prompt) = &mut app.prompt {
            prompt.delete_branch =
                branch_removal == dux_core::worktree_manager::BranchRemoval::Delete;
        } else {
            panic!("the confirmation must be open");
        }
        (app, repo, worktree)
    }

    fn wait_for_status(app: &mut App, needle: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.drain_events();
            let message = app.status.message().clone();
            if message.contains(needle) || std::time::Instant::now() > deadline {
                return message;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn branch_exists(repo: &std::path::Path, branch: &str) -> bool {
        dux_core::test_git::fixture_git()
            .arg("-C")
            .arg(repo)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn removing_a_worktree_with_the_checkbox_on_takes_the_branch_too() {
        let (mut app, repo, worktree) =
            app_with_a_real_worktree(dux_core::worktree_manager::BranchRemoval::Delete);
        app.resolve_confirm_delete_worktree(ConfirmAnswer::Confirm);
        let message = wait_for_status(&mut app, "Removed the worktree");
        assert!(!worktree.exists(), "the worktree directory must be gone");
        assert!(!branch_exists(&repo, "free"), "the branch must be gone");
        assert!(
            message.contains("deleted its branch \"free\""),
            "the final says what happened to the branch: {message}"
        );
    }

    #[test]
    fn removing_a_worktree_with_the_checkbox_off_keeps_the_branch() {
        let (mut app, repo, worktree) =
            app_with_a_real_worktree(dux_core::worktree_manager::BranchRemoval::Keep);
        app.resolve_confirm_delete_worktree(ConfirmAnswer::Confirm);
        let message = wait_for_status(&mut app, "Removed the worktree");
        assert!(!worktree.exists(), "the worktree directory must be gone");
        assert!(branch_exists(&repo, "free"), "the branch must survive");
        assert!(
            message.contains("you did not ask for it"),
            "the branch was kept BY CHOICE here, not by provenance: {message}"
        );
    }

    fn leftover(
        name: &str,
        branches: &[&str],
        provenance: Option<dux_core::model::BranchProvenance>,
        busy: Option<&str>,
    ) -> dux_core::leftover_folders::LeftoverFolder {
        dux_core::leftover_folders::LeftoverFolder {
            folder_present: true,
            path: PathBuf::from(format!("/tmp/worktrees/demo/{name}")),
            branches: branches.iter().map(|b| (*b).to_string()).collect(),
            undeletable: Vec::new(),
            provenance,
            left_by: if provenance.is_some() {
                dux_core::leftover_folders::LeftBy::Agent
            } else {
                dux_core::leftover_folders::LeftBy::Manager
            },
            busy: busy.map(str::to_string),
        }
    }

    /// The manager open on `leftovers` alone, the cursor on the first.
    fn open_manager_on_leftovers(
        app: &mut App,
        leftovers: Vec<dux_core::leftover_folders::LeftoverFolder>,
    ) {
        let mut prompt = manager(Vec::new());
        prompt.project = app.engine.projects[0].clone();
        prompt.selected = crate::app::manager_selectable_rows(&[], &leftovers)
            .into_iter()
            .next();
        prompt.leftovers = leftovers;
        app.prompt = PromptState::ManageWorktrees(prompt);
    }

    fn screen_of(app: &mut App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 40)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A left-over folder is listed under its own header with the branch it
    /// kept and why it is there; a busy one says what is using it instead.
    #[test]
    fn a_left_over_folder_is_listed_with_its_kept_branch_and_why() {
        let mut app = test_app(default_bindings());
        open_manager_on_leftovers(
            &mut app,
            vec![
                leftover("stuck", &["feat"], None, None),
                leftover("busy", &[], None, Some("a terminal open in it")),
            ],
        );
        let screen = screen_of(&mut app);
        assert!(screen.contains("Left Over After A Delete"), "{screen}");
        let line = |name: &str| {
            screen
                .lines()
                .find(|line| line.contains(name))
                .unwrap_or_else(|| panic!("{name} is listed: {screen}"))
                .to_string()
        };
        let stuck = line("stuck");
        assert!(stuck.contains("kept: feat"), "{stuck}");
        assert!(
            stuck.contains("something was still writing into it"),
            "{stuck}"
        );
        let busy = line("busy  ");
        assert!(busy.contains("in use: a terminal open in it"), "{busy}");
        assert!(!busy.contains("kept:"), "no branch, no kept label: {busy}");
        assert!(
            screen.contains("actions"),
            "the confirm hint says what Enter does on this row: {screen}"
        );
    }

    #[test]
    fn enter_on_a_left_over_row_opens_its_actions() {
        let mut app = test_app(default_bindings());
        open_manager_on_leftovers(&mut app, vec![leftover("stuck", &["feat"], None, None)]);
        app.handle_key(key(KeyCode::Enter)).unwrap();
        let PromptState::LeftoverFolderActions(prompt) = &app.prompt else {
            panic!("expected the folder's actions, got {:?}", app.prompt);
        };
        assert_eq!(
            prompt.actions,
            vec![
                LeftoverFolderAction::CopyPath,
                LeftoverFolderAction::DeleteFolder
            ]
        );
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(
            matches!(app.prompt, PromptState::ManageWorktrees(_)),
            "closing the actions puts the manager back, got {:?}",
            app.prompt
        );
    }

    #[test]
    fn a_busy_left_over_folder_offers_copy_path_and_no_delete() {
        let mut app = test_app(default_bindings());
        open_manager_on_leftovers(
            &mut app,
            vec![leftover("stuck", &[], None, Some("a terminal open in it"))],
        );
        app.activate_manager_selection();
        let PromptState::LeftoverFolderActions(prompt) = &app.prompt else {
            panic!("expected the folder's actions, got {:?}", app.prompt);
        };
        assert_eq!(prompt.actions, vec![LeftoverFolderAction::CopyPath]);
    }

    /// The manager open on one left-over folder, its delete confirmation up.
    fn open_leftover_delete(
        app: &mut App,
        branches: &[&str],
        provenance: Option<dux_core::model::BranchProvenance>,
    ) {
        open_manager_on_leftovers(app, vec![leftover("stuck", branches, provenance, None)]);
        app.activate_manager_selection();
        app.handle_key(key(KeyCode::Down)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(
            matches!(app.prompt, PromptState::ConfirmDeleteLeftoverFolder(_)),
            "Delete folder… opens the confirmation, got {:?}",
            app.prompt
        );
    }

    fn leftover_confirm(app: &App) -> &ConfirmDeleteLeftoverFolderPrompt {
        let PromptState::ConfirmDeleteLeftoverFolder(prompt) = &app.prompt else {
            panic!("expected the confirmation, got {:?}", app.prompt);
        };
        prompt
    }

    #[test]
    fn the_branch_box_is_ticked_for_a_dux_branch_and_unticked_for_the_users() {
        use dux_core::model::BranchProvenance;
        for (provenance, ticked) in [
            (None, true),
            (Some(BranchProvenance::CreatedByDux), true),
            (Some(BranchProvenance::AttachedExisting), false),
        ] {
            let mut app = test_app(default_bindings());
            open_leftover_delete(&mut app, &["feat"], provenance);
            let prompt = leftover_confirm(&app);
            assert_eq!(prompt.delete_branches, ticked, "{provenance:?}");
            assert_eq!(prompt.focus, DeleteWorktreeFocus::Cancel);
            let screen = screen_of(&mut app);
            assert_eq!(
                screen.contains("existed before the agent"),
                !ticked,
                "only a branch that predates its agent is warned about: {screen}"
            );
            assert!(
                !screen.contains("unpushed"),
                "no unpushed-commit count here: {screen}"
            );
        }
    }

    #[test]
    fn space_toggles_the_left_over_branch_box_and_escape_abandons() {
        let mut app = test_app(default_bindings());
        open_leftover_delete(&mut app, &["feat"], None);
        app.handle_key(key(KeyCode::Right)).unwrap();
        app.handle_key(key(KeyCode::Right)).unwrap();
        assert_eq!(leftover_confirm(&app).focus, DeleteWorktreeFocus::Checkbox);
        assert!(leftover_confirm(&app).delete_branches);
        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert!(!leftover_confirm(&app).delete_branches);

        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(
            matches!(app.prompt, PromptState::LeftoverFolderActions(_)),
            "Escape steps back to the folder's actions, got {:?}",
            app.prompt
        );
        assert!(
            app.status.message().contains("unchanged"),
            "got {:?}",
            app.status.message()
        );
    }

    /// The rows inside the dialog framed with `title`, each cut to the columns
    /// inside its frame and trimmed, joined with single spaces: a wrapped
    /// sentence reads as one, and a run of spaces inside a row survives.
    fn dialog_rows(screen: &str, title: &str) -> String {
        let grid: Vec<Vec<char>> = screen.lines().map(|line| line.chars().collect()).collect();
        let edge: Vec<char> = format!("\u{256d}{title}").chars().collect();
        let (top, left) = grid
            .iter()
            .enumerate()
            .find_map(|(y, row)| {
                row.windows(edge.len())
                    .position(|window| window == edge.as_slice())
                    .map(|x| (y, x))
            })
            .unwrap_or_else(|| panic!("no dialog titled {title:?}:\n{screen}"));
        let right = (left + 1..grid[top].len())
            .find(|&x| grid[top][x] == '\u{256e}')
            .expect("the dialog's top-right corner");
        let bottom = (top + 1..grid.len())
            .find(|&y| grid[y].get(left) == Some(&'\u{2570}'))
            .expect("the dialog's bottom-left corner");
        (top + 1..bottom)
            .map(|y| grid[y][left + 1..right].iter().collect::<String>())
            .map(|row| row.trim().to_string())
            .filter(|row| !row.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The sentence after the folder's chip in a dialog body.
    fn text_after_the_chip(body: &dux_core::prose::Prose) -> String {
        body.segments()
            .iter()
            .filter_map(|segment| match segment {
                dux_core::prose::ProseSegment::Text(text) => Some(text.trim().to_string()),
                dux_core::prose::ProseSegment::Name { .. } => None,
            })
            .next_back()
            .expect("the body has text after the chip")
    }

    #[test]
    fn a_folder_the_managers_own_removal_left_is_described_as_such_in_one_sentence() {
        let mut app = test_app(default_bindings());
        open_leftover_delete(&mut app, &["feat"], None);
        let text = dialog_rows(&screen_of(&mut app), "Delete Left-Over Folder");
        assert!(
            text.contains(&text_after_the_chip(
                &dux_core::leftover_folders::delete_dialog_body(
                    "/x",
                    dux_core::leftover_folders::FolderOnDisk::Present,
                    dux_core::leftover_folders::LeftBy::Manager,
                )
            )),
            "the shared body, worded for the manager's removal, with no run of spaces: {text}"
        );
        assert!(!text.contains("its agent was deleted"), "{text}");
    }

    #[test]
    fn the_branch_sentence_follows_the_box() {
        let mut app = test_app(default_bindings());
        open_leftover_delete(&mut app, &["feat"], None);
        let text = dialog_rows(&screen_of(&mut app), "Delete Left-Over Folder");
        assert!(text.contains("will be deleted with it"), "{text}");
        app.handle_key(key(KeyCode::Right)).unwrap();
        app.handle_key(key(KeyCode::Right)).unwrap();
        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        let text = dialog_rows(&screen_of(&mut app), "Delete Left-Over Folder");
        assert!(text.contains("is kept."), "{text}");
        assert!(!text.contains("will be deleted with it"), "{text}");
    }

    /// A folder already gone: the row is about its branches, the delete takes
    /// them all with no box to untick, and the dialog says so.
    #[test]
    fn a_gone_folders_row_and_dialog_are_about_its_kept_branches() {
        use dux_core::model::BranchProvenance;
        let mut app = test_app(default_bindings());
        let mut gone = leftover(
            "stuck",
            &["feat", "fix"],
            Some(BranchProvenance::AttachedExisting),
            None,
        );
        gone.folder_present = false;
        open_manager_on_leftovers(&mut app, vec![gone]);
        let screen = screen_of(&mut app);
        let row = screen
            .lines()
            .find(|line| line.contains("stuck"))
            .unwrap_or_else(|| panic!("listed: {screen}"));
        assert!(
            row.contains("folder already removed; branches kept"),
            "{row}"
        );
        assert!(!row.contains("still writing"), "{row}");

        app.activate_manager_selection();
        let screen = screen_of(&mut app);
        assert!(screen.contains("Delete branches…"), "{screen}");
        assert!(!screen.contains("Delete folder"), "{screen}");
        app.handle_key(key(KeyCode::Down)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();

        let prompt = leftover_confirm(&app);
        assert!(!prompt.has_branch_checkbox(), "nothing to untick");
        assert!(prompt.delete_branches, "the delete IS the branches' delete");
        let screen = screen_of(&mut app);
        let text = dialog_rows(&screen, "Delete Kept Branches");
        assert!(text.contains("Delete the kept branches?"), "{text}");
        assert!(
            text.contains(&text_after_the_chip(
                &dux_core::leftover_folders::delete_dialog_body(
                    "/x",
                    dux_core::leftover_folders::FolderOnDisk::Gone,
                    dux_core::leftover_folders::LeftBy::Agent,
                )
            )),
            "{text}"
        );
        assert!(text.contains("will be deleted, unless"), "{text}");
        assert!(text.contains("existed before the agent"), "{text}");
        assert!(text.contains("Delete branches"), "{text}");
        assert!(!text.contains("Delete folder"), "{text}");
        assert!(!text.contains("Also delete"), "no checkbox: {text}");
    }

    #[test]
    fn a_gone_folder_with_one_branch_says_branch_in_the_singular() {
        let mut app = test_app(default_bindings());
        let mut gone = leftover("stuck", &["feat"], None, None);
        gone.folder_present = false;
        open_manager_on_leftovers(&mut app, vec![gone]);
        assert!(
            screen_of(&mut app).contains("folder already removed; branch kept"),
            "singular row note"
        );
        app.activate_manager_selection();
        assert!(screen_of(&mut app).contains("Delete branch…"));
    }

    /// A sentence's words with every run of spaces as one, which is how it
    /// reads on screen whatever the chips' padding and the wrapping did.
    fn squashed(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn spoken(prose: &dux_core::prose::Prose) -> String {
        squashed(
            &prose
                .segments()
                .iter()
                .map(|segment| match segment {
                    dux_core::prose::ProseSegment::Text(text) => text.as_str(),
                    dux_core::prose::ProseSegment::Name { name, .. } => name.as_str(),
                })
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    fn undeletable(
        name: &str,
        why: dux_core::leftover_folders::BranchBlock,
    ) -> dux_core::leftover_folders::UndeletableBranch {
        dux_core::leftover_folders::UndeletableBranch {
            name: name.to_string(),
            why,
        }
    }

    /// A gone folder whose one kept branch a delete cannot remove has nothing
    /// to delete: its actions are Copy path alone, saying why the branch is
    /// kept and how to delete it yourself, and its row still names it.
    #[test]
    fn a_gone_folder_whose_branch_cannot_be_deleted_offers_only_copy_path_and_says_why() {
        use dux_core::leftover_folders::BranchBlock;
        let mut app = test_app(default_bindings());
        let mut gone = leftover("stuck", &[], None, None);
        gone.folder_present = false;
        gone.undeletable = vec![undeletable("feat", BranchBlock::Moved)];
        open_manager_on_leftovers(&mut app, vec![gone]);
        let screen = screen_of(&mut app);
        let row = screen
            .lines()
            .find(|line| line.contains("stuck"))
            .unwrap_or_else(|| panic!("listed: {screen}"));
        assert!(row.contains("kept: feat"), "{row}");

        app.activate_manager_selection();
        let PromptState::LeftoverFolderActions(prompt) = &app.prompt else {
            panic!("expected the folder's actions, got {:?}", app.prompt);
        };
        assert_eq!(prompt.actions, vec![LeftoverFolderAction::CopyPath]);
        let text = squashed(&dialog_rows(&screen_of(&mut app), "Left-Over Folder"));
        let sentence = spoken(&dux_core::leftover_folders::kept_branch_sentence(
            "feat",
            &BranchBlock::Moved,
        ));
        assert!(text.contains(&sentence), "{sentence:?} in {text}");
    }

    /// A present folder with one branch a delete can remove and one it
    /// cannot: the box and the fate sentence name only the first, and the
    /// other gets its own kept sentence.
    #[test]
    fn the_delete_dialog_boxes_only_the_deletable_branch_and_says_why_the_other_is_kept() {
        use dux_core::leftover_folders::BranchBlock;
        let mut app = test_app(default_bindings());
        let held = BranchBlock::CheckedOut {
            path: "~/elsewhere".to_string(),
        };
        let mut folder = leftover("stuck", &["free"], None, None);
        folder.undeletable = vec![undeletable("held", held.clone())];
        open_manager_on_leftovers(&mut app, vec![folder]);
        app.activate_manager_selection();
        app.handle_key(key(KeyCode::Down)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(leftover_confirm(&app).has_branch_checkbox());
        let text = squashed(&dialog_rows(
            &screen_of(&mut app),
            "Delete Left-Over Folder",
        ));
        assert!(text.contains("Also delete the branch free"), "{text}");
        assert!(!text.contains("branches"), "one deletable branch: {text}");
        assert!(
            text.contains("The branch free will be deleted with it"),
            "{text}"
        );
        let sentence = spoken(&dux_core::leftover_folders::kept_branch_sentence(
            "held", &held,
        ));
        assert!(text.contains(&sentence), "{sentence:?} in {text}");
    }

    /// A present folder whose only kept branch cannot be deleted gets no box.
    #[test]
    fn a_folder_with_no_deletable_branch_has_no_box() {
        let mut app = test_app(default_bindings());
        let mut folder = leftover("stuck", &[], None, None);
        folder.undeletable = vec![undeletable(
            "held",
            dux_core::leftover_folders::BranchBlock::NoCommit,
        )];
        open_manager_on_leftovers(&mut app, vec![folder]);
        app.activate_manager_selection();
        app.handle_key(key(KeyCode::Down)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(!leftover_confirm(&app).has_branch_checkbox());
        let text = squashed(&dialog_rows(
            &screen_of(&mut app),
            "Delete Left-Over Folder",
        ));
        assert!(!text.contains("Also delete"), "{text}");
        assert!(text.contains("The branch held is kept:"), "{text}");
    }

    /// A fresh listing of the manager landing with `leftovers`.
    fn deliver_listing(app: &mut App, leftovers: Vec<dux_core::leftover_folders::LeftoverFolder>) {
        let project_id = app.engine.projects[0].id.clone();
        app.apply_reaction(
            dux_core::engine::EventReaction::ManageableWorktreesArrived {
                project_id,
                result: Ok(Vec::new()),
                leftovers,
                status_op_id: None,
            },
        );
    }

    /// The confirmation acts on the row it was opened from: a fresh listing
    /// whose row for that folder differs closes it back onto the refreshed
    /// manager, saying so.
    #[test]
    fn a_relisting_that_changes_the_folders_row_closes_its_confirmation() {
        let mut app = test_app(default_bindings());
        open_leftover_delete(&mut app, &["feat"], None);
        deliver_listing(
            &mut app,
            vec![leftover("stuck", &["feat", "fix"], None, None)],
        );
        let PromptState::ManageWorktrees(prompt) = &app.prompt else {
            panic!("expected the refreshed manager, got {:?}", app.prompt);
        };
        assert_eq!(prompt.leftovers[0].branches, vec!["feat", "fix"]);
        assert!(
            app.status.message().contains("Look again"),
            "got {:?}",
            app.status.message()
        );
    }

    #[test]
    fn a_relisting_with_the_same_row_keeps_the_confirmation_open() {
        let mut app = test_app(default_bindings());
        open_leftover_delete(&mut app, &["feat"], None);
        deliver_listing(&mut app, vec![leftover("stuck", &["feat"], None, None)]);
        assert!(
            matches!(app.prompt, PromptState::ConfirmDeleteLeftoverFolder(_)),
            "got {:?}",
            app.prompt
        );
    }

    #[test]
    fn a_relisting_without_the_folder_closes_its_actions() {
        let mut app = test_app(default_bindings());
        open_manager_on_leftovers(&mut app, vec![leftover("stuck", &["feat"], None, None)]);
        app.activate_manager_selection();
        deliver_listing(&mut app, Vec::new());
        let PromptState::ManageWorktrees(prompt) = &app.prompt else {
            panic!("expected the refreshed manager, got {:?}", app.prompt);
        };
        assert!(prompt.leftovers.is_empty(), "{:?}", prompt.leftovers);
    }

    #[test]
    fn a_busy_folders_actions_say_the_shared_in_use_sentence() {
        let mut app = test_app(default_bindings());
        open_manager_on_leftovers(
            &mut app,
            vec![leftover("stuck", &[], None, Some("a terminal open in it"))],
        );
        app.activate_manager_selection();
        let screen = screen_of(&mut app);
        assert!(
            screen.contains(&dux_core::leftover_folders::in_use_sentence(
                "a terminal open in it"
            )),
            "{screen}"
        );
    }

    /// The whole journey against a real repository: a worktree whose folder
    /// git cannot empty (a read-only subdirectory holding a file) is removed
    /// from the manager, which leaves the folder and keeps its branch; the
    /// relisted manager shows it, and its Delete folder takes the folder and
    /// the ticked branch.
    #[test]
    fn deleting_a_left_over_folder_from_the_manager_takes_it_and_its_branch() {
        use std::os::unix::fs::PermissionsExt;
        // Root ignores the read-only mode, so git would empty the folder.
        if std::process::Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
        {
            return;
        }
        let (mut app, repo, worktree) =
            app_with_a_real_worktree(dux_core::worktree_manager::BranchRemoval::Delete);
        let locked = worktree.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("held.txt"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        app.resolve_confirm_delete_worktree(ConfirmAnswer::Confirm);
        let message = wait_for_status(&mut app, "something kept writing");
        // Restore the mode whatever happens next, so the fixture can be cleaned.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(message.contains("something kept writing"), "{message}");
        assert!(worktree.exists(), "git could not empty the folder");
        assert!(branch_exists(&repo, "free"), "the branch is kept with it");

        let project = app.engine.projects[0].clone();
        app.begin_manage_worktrees_for_project(project, None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !matches!(&app.prompt, PromptState::ManageWorktrees(p) if !p.loading)
            && std::time::Instant::now() < deadline
        {
            app.drain_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let PromptState::ManageWorktrees(prompt) = &app.prompt else {
            panic!("the manager must be open, got {:?}", app.prompt);
        };
        assert_eq!(prompt.leftovers.len(), 1, "{:?}", prompt.leftovers);
        assert_eq!(prompt.leftovers[0].branches, vec!["free".to_string()]);
        assert_eq!(prompt.selected, Some(ManagerSelection::Leftover(0)));

        app.handle_key(key(KeyCode::Enter)).unwrap();
        app.handle_key(key(KeyCode::Down)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(leftover_confirm(&app).delete_branches);
        app.handle_key(key(KeyCode::Right)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.prompt, PromptState::None), "{:?}", app.prompt);
        let message = wait_for_status(&mut app, "Deleted the left-over folder");
        assert!(message.contains("and its branch"), "{message}");
        assert!(!worktree.exists(), "the folder is gone");
        assert!(!branch_exists(&repo, "free"), "the ticked branch is gone");
    }

    #[test]
    fn the_checkbox_label_names_the_branch() {
        assert_eq!(
            crate::app::render::delete_worktree_checkbox_label(Some("feature-x")),
            "Also delete the branch feature-x"
        );
    }
}
