//! The TUI worktree manager: the manual override for removing a worktree, and
//! the branch with it.
//!
//! Deleting an agent keeps a branch dux did not create (see
//! [`dux_core::model::BranchProvenance`]). This surface is where the user says
//! "delete that one anyway": its checkbox is honored whatever the branch's
//! origin, exactly as the web's Worktrees dialog is, because the user is
//! pointing at one specific worktree.
//!
//! The rules (which worktrees the manager owns, which are removable, what a
//! removal does to the branch) all live in
//! [`dux_core::worktree_manager`], shared with the web route so the two
//! surfaces cannot drift. What lives here is the TUI's half: opening the
//! picker, opening the confirmation, and dispatching the removal onto a
//! background worker with a keyed status.

use super::*;

impl App {
    /// `manage-worktrees`: open the project chooser, then (per project) load
    /// that project's manageable worktrees into [`PromptState::ManageWorktrees`].
    pub(crate) fn manage_project_worktrees(&mut self) -> Result<()> {
        self.open_project_chooser(ProjectChooserIntent::ManageWorktrees)
    }

    /// Per-project body for `ManageWorktrees`: opens the manager and spawns the
    /// listing worker.
    pub(crate) fn begin_manage_worktrees_for_project(&mut self, project: Project) -> Result<()> {
        if project.path_missing {
            self.prompt = PromptState::None;
            self.set_warning(format!("Project path not found: {}", project.path));
            return Ok(());
        }

        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ManageWorktrees(ManageWorktreesPrompt {
            project: project.clone(),
            entries: Vec::new(),
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
        Ok(())
    }

    /// Open the removal confirmation for the manager's selected row.
    ///
    /// A row an agent holds is refused here with the same sentence the web
    /// route answers its 409 with: the worktree is still listed (hiding it
    /// would leave the user hunting for a directory they can see), but the
    /// supported route is deleting the agent.
    pub(crate) fn confirm_delete_selected_worktree(&mut self) -> Result<()> {
        let PromptState::ManageWorktrees(prompt) = &self.prompt else {
            return Ok(());
        };
        let Some(entry) = prompt.selected.and_then(|index| prompt.entries.get(index)) else {
            self.set_error("No worktree is selected.");
            return Ok(());
        };
        if !entry.is_removable() {
            self.set_error(
                "An agent is working in that directory; delete that agent first. If it is a \
                 standalone agent, deleting it leaves the directory in place, so remove that one \
                 yourself.",
            );
            return Ok(());
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
        Ok(())
    }

    /// Resolve the removal confirmation. Cancelling returns to the manager
    /// (nothing was touched); confirming closes every overlay and dispatches
    /// the removal.
    ///
    /// Confirming deliberately does NOT put the manager back, even though the
    /// user may well want to remove a second worktree, and the web's dialog
    /// (which closes onto a list that is still there) is not the same shape.
    /// The reason is the status line. The removal opens a keyed Busy and
    /// finishes with a verbose final saying what actually happened to the
    /// branch, and that report is the whole point of the operation; relisting
    /// would immediately open a SECOND keyed Busy for the fresh listing, and
    /// on a single most-recent-wins status line the listing's chatter would
    /// bury the removal's answer. The user reopens the manager when they want
    /// another one, and it lists what is actually there now.
    pub(super) fn resolve_confirm_delete_worktree(&mut self, confirm: bool) -> bool {
        let PromptState::ConfirmDeleteWorktree(prompt) = &self.prompt else {
            return false;
        };
        let prompt = prompt.clone();
        if !confirm {
            self.prompt = PromptState::ManageWorktrees(prompt.previous);
            self.set_info("Removal cancelled. The worktree and its branch are unchanged.");
            return false;
        }
        self.prompt = PromptState::None;
        self.dispatch_worktree_removal(&prompt);
        false
    }

    /// Run the removal on a background worker, with a keyed Busy and a verbose
    /// final that says what actually happened to the branch (never what the
    /// checkbox asked for: `git branch -D` can refuse).
    fn dispatch_worktree_removal(&mut self, prompt: &ConfirmDeleteWorktreePrompt) {
        let project = prompt.project.clone();
        let paths = self.engine.paths.clone();
        let sessions = self.engine.sessions.clone();
        let path = prompt.path.clone();
        let delete_branch = prompt.delete_branch;
        let display_path = path.to_string_lossy().to_string();
        let failure_path = display_path.clone();
        let op = dux_core::engine::status_op(format!("Removing the worktree at {display_path}..."))
            .on_success(move |outcome: &dux_core::worktree_manager::RemovalOutcome| match outcome {
                // Both refusals are decided in core against a listing taken at
                // removal time, so they can differ from what the manager showed
                // a moment ago; say which one happened rather than "failed".
                dux_core::worktree_manager::RemovalOutcome::NotManaged => {
                    dux_core::engine::Final::warning(format!(
                        "Nothing was removed: {display_path} is no longer a worktree dux manages \
                         for this project. Reopen the worktree manager to see the current list."
                    ))
                }
                dux_core::worktree_manager::RemovalOutcome::Attached => {
                    dux_core::engine::Final::warning(format!(
                        "Nothing was removed: an agent is attached to {display_path}. Delete that \
                         agent first. Deleting a managed agent removes its worktree with it; \
                         deleting a standalone agent leaves its directory in place, so remove \
                         that one yourself."
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
            })
            .on_failure(move |error: &String| {
                dux_core::engine::Final::error(format!(
                    "Could not remove the worktree at {failure_path}: {error}"
                ))
            });
        let reaction = self.engine.spawn_status_op(op, move || {
            dux_core::worktree_manager::remove_managed_worktree(
                &project,
                &paths,
                &sessions,
                &path,
                delete_branch,
            )
        });
        self.apply_reaction(reaction);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use dux_core::worktree_manager::ManagedWorktree;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

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
        app.begin_manage_worktrees_for_project(project)
            .expect("opening the manager succeeds");

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

    fn row(name: &str, branch: Option<&str>, agent: Option<&str>, dirty: bool) -> ManagedWorktree {
        ManagedWorktree {
            path: PathBuf::from(format!("/tmp/worktrees/demo/{name}")),
            label: branch.unwrap_or("detached abc1234").to_string(),
            branch: branch.map(str::to_string),
            dirty,
            attached_session_id: agent.map(str::to_string),
        }
    }

    fn manager(entries: Vec<ManagedWorktree>) -> ManageWorktreesPrompt {
        let mut app = test_app(default_bindings());
        let project = app.engine.projects[0].clone();
        app.prompt = PromptState::None;
        ManageWorktreesPrompt {
            project,
            selected: removable_worktree_indices(&entries).into_iter().next(),
            entries,
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
            row("held", Some("held"), Some("session-1"), false),
            row("free", Some("free"), None, false),
        ];
        assert_eq!(removable_worktree_indices(&entries), vec![1]);
        let rows = manage_worktree_visual_rows(&entries, false, None);
        assert!(
            rows.iter()
                .any(|r| matches!(r, ManageWorktreeVisualRow::Entry(0))),
            "an attached worktree is listed, not hidden: {rows:?}"
        );
    }

    #[test]
    fn picking_a_free_worktree_opens_the_confirmation_with_the_branch_checkbox_on() {
        let mut app = test_app(default_bindings());
        open_manager(&mut app, vec![row("free", Some("free"), None, false)]);
        app.confirm_delete_selected_worktree().unwrap();
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
        let entries = vec![row("held", Some("held"), Some("session-1"), false)];
        let mut prompt = manager(entries);
        prompt.selected = Some(0);
        app.prompt = PromptState::ManageWorktrees(prompt);
        app.confirm_delete_selected_worktree().unwrap();
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
        open_manager(&mut app, vec![row("loose", None, None, false)]);
        app.confirm_delete_selected_worktree().unwrap();
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
        open_manager(&mut app, vec![row("free", Some("free"), None, false)]);
        app.confirm_delete_selected_worktree().unwrap();
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
        open_manager(&mut app, vec![row("loose", None, None, false)]);
        app.confirm_delete_selected_worktree().unwrap();
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
        open_manager(&mut app, vec![row("free", Some("free"), None, false)]);
        app.confirm_delete_selected_worktree().unwrap();
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
        open_manager(&mut app, vec![row("free", Some("free"), None, false)]);
        app.confirm_delete_selected_worktree().unwrap();
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
        open_manager(&mut app, vec![row("free", Some("free"), None, false)]);
        app.confirm_delete_selected_worktree().unwrap();
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
    fn app_with_a_real_worktree(delete_branch: bool) -> (App, PathBuf, PathBuf) {
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
        app.begin_manage_worktrees_for_project(project).unwrap();
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

        app.confirm_delete_selected_worktree().unwrap();
        if let PromptState::ConfirmDeleteWorktree(prompt) = &mut app.prompt {
            prompt.delete_branch = delete_branch;
        } else {
            panic!("the confirmation must be open");
        }
        (app, repo, worktree)
    }

    fn wait_for_status(app: &mut App, needle: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.drain_events();
            let message = app.status.message().to_string();
            if message.contains(needle) || std::time::Instant::now() > deadline {
                return message;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn branch_exists(repo: &std::path::Path, branch: &str) -> bool {
        std::process::Command::new("git")
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
        let (mut app, repo, worktree) = app_with_a_real_worktree(true);
        app.resolve_confirm_delete_worktree(true);
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
        let (mut app, repo, worktree) = app_with_a_real_worktree(false);
        app.resolve_confirm_delete_worktree(true);
        let message = wait_for_status(&mut app, "Removed the worktree");
        assert!(!worktree.exists(), "the worktree directory must be gone");
        assert!(branch_exists(&repo, "free"), "the branch must survive");
        assert!(
            message.contains("you did not ask for it"),
            "the branch was kept BY CHOICE here, not by provenance: {message}"
        );
    }

    #[test]
    fn the_checkbox_label_names_the_branch() {
        assert_eq!(
            crate::app::render::delete_worktree_checkbox_label(Some("feature-x")),
            "Also delete the branch feature-x"
        );
    }
}
