//! "Change base branch": switch a project's folder to a branch the user picked
//! and make it the base new worktrees start from. One entry point for both
//! surfaces ([`Engine::change_project_base_branch`]); the git work runs in a
//! worker (`crate::base_branch`), and its answer lands in
//! [`Engine::process_project_base_branch_changed`], which saves the base and
//! resolves the op into the final both surfaces show.

use super::*;
use crate::model::ProjectBranchStatus;

impl Engine {
    /// Start changing the base branch of project `project_id` to `branch`.
    ///
    /// Returns the status to show now: the op's keyed busy, or an ordinary
    /// warning when another operation already holds the project folder (see
    /// [`ProjectFolderAction`]), in which case nothing started. The outcome
    /// arrives later as the same op's final, through
    /// `WorkerEvent::ProjectBaseBranchChanged`.
    ///
    /// `Err` only for what can be answered without git: an unknown project, a
    /// folder already known to be missing, an empty branch. Whether `branch`
    /// exists, is a valid branch name and is free is decided in the worker,
    /// because git must not run on a surface's thread.
    pub fn change_project_base_branch(
        &mut self,
        project_id: &str,
        branch: &str,
    ) -> anyhow::Result<StatusUpdate> {
        let project = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
        if project.path_missing {
            anyhow::bail!(
                change_base_branch_path_missing_message(&project.name)
                    .message()
                    .to_string()
            );
        }
        if branch.trim().is_empty() {
            anyhow::bail!(
                "Choose a branch to change the base of project \"{}\" to.",
                project.name
            );
        }
        if let Err(refusal) = self.begin_project_folder_action(
            &project.path,
            &project.name,
            ProjectFolderAction::ChangeBaseBranch,
        ) {
            return Ok(refusal);
        }

        let project_name = project.name.clone();
        let op = status_op(change_base_branch_busy_message(&project.name, branch))
            .resolve_in_handler(move |outcome: &ChangeBaseOutcome| {
                change_base_final(&project_name, outcome)
            })
            .with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = self.begin_status_op(&op);
        self.pending_change_base_ops.insert(op_id.clone(), op);

        let worker_tx = self.worker_tx.clone();
        let branch = branch.to_string();
        let guard = self.checkout_move_guard();
        // Matched against the holder in the worker, which canonicalizes: that
        // must not run on the engine's thread.
        let holders = self.holder_context_for_base_switch(&project);
        let spawned = thread::Builder::new()
            .name("dux-change-base-branch".to_string())
            .spawn(move || {
                // A panic still answers: the busy is on screen and the folder
                // is locked, and only this event releases both.
                let result = switch_reporting_panics(|| {
                    crate::base_branch::switch_to_base_branch(
                        Path::new(&project.path),
                        &branch,
                        &guard,
                        &holders,
                    )
                });
                let _ = worker_tx.send(WorkerEvent::ProjectBaseBranchChanged {
                    project,
                    branch,
                    result,
                    status_op_id: op_id,
                });
            });
        if let Err(error) = spawned {
            let key = pending.key.clone().unwrap_or_default();
            if let Some(path) = self
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| project.path.clone())
            {
                self.end_project_folder_action(&path);
            }
            self.abandon_status_op(&key);
            return Ok(StatusUpdate::error(format!(
                "Could not start changing the base branch: {error}"
            ))
            .with_key(key));
        }
        Ok(pending)
    }

    /// What the change worker needs to say who holds a branch: every agent's
    /// directory with its row label and kind, and the project's managed
    /// worktree area. Only gathered here; the canonical comparison runs in the
    /// worker.
    fn holder_context_for_base_switch(
        &self,
        project: &Project,
    ) -> crate::base_branch::HolderContext {
        let agents = self
            .sessions
            .iter()
            .map(|session| crate::base_branch::AgentDirectory {
                directory: session.directory().to_string(),
                label: session.display_label(),
                // Deleting a standalone agent never removes its folder, so the
                // refusal must not offer that as a way to free the branch.
                standalone: match &session.workspace {
                    crate::model::AgentWorkspace::Managed(_) => false,
                    crate::model::AgentWorkspace::Folder(_) => true,
                },
            })
            .collect();
        crate::base_branch::HolderContext {
            agents,
            managed_root: self.paths.worktrees_root.join(&project.name),
        }
    }

    /// The change worker's answer: release the folder, save the base when the
    /// folder is on the new branch, and resolve the op into its final.
    pub(crate) fn process_project_base_branch_changed(
        &mut self,
        project: Project,
        branch: String,
        result: Result<
            crate::base_branch::BaseBranchSwitched,
            crate::base_branch::BaseBranchChangeFailure,
        >,
        status_op_id: String,
    ) -> EventReaction {
        self.end_project_folder_action(&project.path);
        let outcome = match result {
            Ok(switched) => {
                if let Some(existing) = self.projects.iter_mut().find(|p| p.id == project.id) {
                    existing.current_branch = branch.clone();
                    existing.branch_status = ProjectBranchStatus::Leading;
                }
                let base = self.adopt_project_base(&project, &branch);
                ChangeBaseOutcome::Changed {
                    branch,
                    folder_was_on_it: switched.folder_was_on_it,
                    base,
                }
            }
            Err(failure) => {
                if let crate::base_branch::BaseBranchChangeFailure::SwitchFailed(reason)
                | crate::base_branch::BaseBranchChangeFailure::ListFailed(reason) = &failure
                {
                    crate::logger::error(&format!(
                        "changing the base branch of {} to {branch} failed: {reason}",
                        project.path
                    ));
                }
                ChangeBaseOutcome::Refused {
                    branch,
                    failure,
                    repo_path: project.path.clone(),
                }
            }
        };
        match self.pending_change_base_ops.remove(&status_op_id) {
            Some(op) => op.resolve(&outcome).into_reaction(),
            // Unreachable while the op is registered before the worker starts;
            // answered on the same key rather than dropped all the same.
            None => {
                let final_ = change_base_final(&project.name, &outcome);
                ResolvedFinal::new(status_op_id, final_).into_reaction()
            }
        }
    }
}

/// Run the change worker's switch, answering a panic as a failed switch so the
/// op still gets its final and the folder is released, and logging what the
/// panic said.
fn switch_reporting_panics(
    switch: impl FnOnce() -> Result<
        crate::base_branch::BaseBranchSwitched,
        crate::base_branch::BaseBranchChangeFailure,
    >,
) -> Result<crate::base_branch::BaseBranchSwitched, crate::base_branch::BaseBranchChangeFailure> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(switch)).unwrap_or_else(|payload| {
        let reason = crate::engine::format_panic_payload(payload);
        crate::logger::error(&format!("change-base-branch worker panicked: {reason}"));
        Err(crate::base_branch::BaseBranchChangeFailure::SwitchFailed(
            format!("the change-base-branch worker panicked: {reason}"),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_session, sample_standalone_session, test_engine};

    /// A panicking switch still answers as a failed switch, and both the log
    /// and the answer carry what the panic said.
    #[test]
    fn a_panicking_switch_answers_with_the_panic_payload() {
        let (result, logged) = crate::logger::capture_for_test(|| {
            switch_reporting_panics(|| panic!("the listing exploded"))
        });
        assert_eq!(
            logged,
            ["ERROR change-base-branch worker panicked: the listing exploded"]
        );
        let reason = match result {
            Err(crate::base_branch::BaseBranchChangeFailure::SwitchFailed(reason)) => reason,
            other => panic!("expected a failed switch: {other:?}"),
        };
        assert!(
            reason.contains("the listing exploded"),
            "the payload is kept: {reason}"
        );
    }

    /// The worker is handed every agent's directory under its row label, a
    /// standalone one marked as such (deleting it never frees a branch its
    /// folder holds), and the project's own managed worktree area.
    #[test]
    fn the_holder_context_marks_standalone_agents_and_scopes_the_managed_area() {
        let (mut engine, _tmp) = test_engine();
        engine
            .sessions
            .push(sample_session("s-managed", "p1", "fix-login"));
        engine
            .sessions
            .push(sample_standalone_session("s-folder", "/home/me/notes"));
        let project = crate::engine::test_support::sample_project("p1", "/work/app");

        let holders = engine.holder_context_for_base_switch(&project);

        assert_eq!(
            holders.agents,
            vec![
                crate::base_branch::AgentDirectory {
                    directory: "/tmp/s-managed-worktree".to_string(),
                    label: "s-managed-title".to_string(),
                    standalone: false,
                },
                crate::base_branch::AgentDirectory {
                    directory: "/home/me/notes".to_string(),
                    label: "s-folder-title".to_string(),
                    standalone: true,
                },
            ]
        );
        assert_eq!(
            holders.managed_root,
            engine.paths.worktrees_root.join(&project.name)
        );
    }
}
