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
        let spawned = thread::Builder::new()
            .name("dux-change-base-branch".to_string())
            .spawn(move || {
                // A panic still answers: the busy is on screen and the folder
                // is locked, and only this event releases both.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::base_branch::switch_to_base_branch(
                        Path::new(&project.path),
                        &branch,
                        &guard,
                    )
                }))
                .unwrap_or_else(|_| {
                    Err(crate::base_branch::BaseBranchChangeFailure::SwitchFailed(
                        "the change-base-branch worker panicked".to_string(),
                    ))
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
