//! The engine's half of cloning a repository as a project with its first
//! agent: see [`crate::clone_project`] for the whole flow.

use std::path::PathBuf;
use std::time::Duration;

use super::{Command, Engine, EventReaction, InFlightKey, StatusUpdate};
use crate::clone_project::{
    CloneJob, CloneOutcome, CloneRequest, ClonedProject, ClonedRepository, PendingClone,
};
use crate::status_text::StatusText;
use crate::worker::CreateAgentRequest;

impl Engine {
    /// Start a clone: run the checks that need neither git nor the
    /// destination, take the destination's in-flight key and hold, and spawn
    /// the worker. Answers the clone's keyed busy, or the sentence of the check
    /// that refused it, in which case nothing was started. `from_web` says a
    /// browser asked, whose follow-up the web layer drives.
    pub fn begin_clone_project(
        &mut self,
        request: &CloneRequest,
        from_web: bool,
    ) -> Result<StatusUpdate, StatusText> {
        let projects: Vec<&str> = self.projects.iter().map(|p| p.path.as_str()).collect();
        let prepared =
            crate::clone_project::prepare_clone(request, home::home_dir().as_deref(), &projects)?;
        let destination = prepared.destination;
        let key = InFlightKey::Clone(destination.clone());
        if !self.mark_in_flight(key.clone()) {
            return Err(crate::status_text![
                "dux is already cloning a repository into ",
                n(destination.display()),
                " or setting one up there. Wait for it to finish, or pick another destination."
            ]);
        }
        if let Err(refused) = self.hold_path_for_in_flight(
            &key,
            &destination,
            crate::worktree_ops::WorktreeOpKind::CloneRepository,
        ) {
            self.clear_in_flight(&key);
            return Err(refused.sentence("clone a repository there"));
        }
        let address = crate::clone_project::redact_remote_userinfo(&prepared.url);
        let busy = crate::status_text![
            "Cloning ",
            n(address),
            " into ",
            n(destination.display()),
            "..."
        ];
        let op = crate::engine::status_op(busy)
            .resolve_in_handler(|outcome: &CloneOutcome| outcome.final_status())
            .with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = self.begin_status_op(&op);
        self.clones
            .pending
            .insert(op_id.clone(), PendingClone { op, from_web });
        let job = CloneJob {
            status_op_id: op_id.clone(),
            url: prepared.url,
            destination: destination.clone(),
            agent_name: prepared.agent_name.clone(),
            stall: Duration::from_secs(self.config.git.clone_stall_seconds.max(1)),
            processes: self.clones.processes.clone(),
        };
        let worker_tx = self.worker_tx.clone();
        let agent_name = prepared.agent_name;
        std::thread::spawn(move || {
            // The key and the op must end even if the job panics.
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::clone_project::run_clone_job(job, &worker_tx);
            }))
            .is_err();
            if panicked {
                let _ = worker_tx.send(crate::worker::WorkerEvent::RepositoryCloned {
                    status_op_id: Some(op_id),
                    path: destination,
                    agent_name,
                    result: Err(CloneOutcome::Failed(
                        "The clone worker stopped unexpectedly, so the clone may not have \
                         finished. Nothing was added to the workspace."
                            .into(),
                    )),
                });
            }
        });
        Ok(pending)
    }

    /// Whether a clone is running into `path`, so a request to set a
    /// repository up there can say what it waits for.
    pub fn clone_running_into(&self, path: &std::path::Path) -> bool {
        self.is_in_flight(&InFlightKey::Clone(path.to_path_buf()))
    }

    /// A clone's worker finished: free the destination, and end the operation
    /// when the clone stopped short of a project, or hand a ready clone to the
    /// surface that adds it.
    pub(crate) fn process_repository_cloned(
        &mut self,
        status_op_id: Option<String>,
        path: PathBuf,
        agent_name: String,
        result: Result<ClonedRepository, CloneOutcome>,
    ) -> EventReaction {
        self.clear_in_flight(&InFlightKey::Clone(path.clone()));
        match result {
            Ok(cloned) => EventReaction::AddProjectAfterClone(Box::new(ClonedProject {
                status_op_id,
                path,
                agent_name,
                cloned,
            })),
            Err(outcome) => match status_op_id.and_then(|id| self.clones.pending.remove(&id)) {
                Some(pending) => pending.op.resolve(&outcome).into_reaction(),
                None => match outcome {
                    CloneOutcome::Refused(text) | CloneOutcome::Failed(text) => {
                        EventReaction::Status(StatusUpdate::error(text))
                    }
                    CloneOutcome::Kept(text) => {
                        EventReaction::Status(StatusUpdate::warning(text).sticky())
                    }
                    CloneOutcome::HandedOff => EventReaction::Nothing,
                },
            },
        }
    }

    /// Add a ready clone as a project and dispatch its agent create, ending the
    /// clone's operation: handed off to the create, whose final is the one the
    /// user sees, or kept, saying what is done and what is missing. The one
    /// place either surface does this, so it happens once whichever drives it.
    pub fn finish_clone(&mut self, done: &ClonedProject) -> EventReaction {
        let Some((op_id, pending)) = done
            .status_op_id
            .as_ref()
            .and_then(|id| self.clones.pending.remove_entry(id))
        else {
            return EventReaction::Nothing;
        };
        let address = &done.cloned.address;
        let path = done.path.to_string_lossy().into_owned();
        let display_name = crate::wire::display_project_name("", &path);
        let added = self.add_project_without_op(
            crate::wire::ProjectAdd {
                path: &path,
                display_name: display_name.clone(),
                current_branch: &done.cloned.branch,
                leading_branch: &done.cloned.leading_branch,
                status_message: crate::status_text![
                    "Added project ",
                    q(display_name),
                    " to the workspace."
                ],
                add_failed_prefix: "Couldn't add the project",
            },
            pending.op.scope().clone(),
            Some(&op_id),
        );
        let Some(project) = added
            .added
            .as_ref()
            .and_then(|(id, _)| self.projects.iter().find(|p| &p.id == id).cloned())
        else {
            let reason = added.failure(&"the add did not say why.".into());
            return pending
                .op
                .resolve(&CloneOutcome::Kept(crate::status_text![
                    "Cloned ",
                    n(address),
                    " into ",
                    n(path),
                    ", but couldn't add it as a project: ",
                    reason,
                    " The clone is still there, and no agent was created."
                ]))
                .into_reaction();
        };
        let agent = &done.agent_name;
        let cloned = crate::clone_project::ClonedFor {
            address: address.clone(),
            path: done.path.clone(),
            project_name: project.name.clone(),
            agent_name: agent.clone(),
        };
        let not_created = |reason: StatusText| CloneOutcome::Kept(cloned.agent_missing(reason));
        if done.cloned.name_taken {
            let kept = not_created(
                "a branch with that name already exists in the clone. Pick another name, or \
                 use New agent, which offers to attach to that branch."
                    .into(),
            );
            return EventReaction::Multi(vec![
                EventReaction::RebuildLeftItems,
                pending.op.resolve(&kept).into_reaction(),
            ]);
        }
        let request = CreateAgentRequest::NewProject {
            project: project.clone(),
            custom_name: Some(agent.clone()),
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: false,
            cloned: Some(cloned.clone()),
        };
        let busy_message = crate::status_text![
            "Creating agent ",
            q(agent),
            " in project ",
            q(project.name),
            " and launching a fresh session..."
        ];
        // The create mints its busy from `current_origin`, so it reaches the
        // connection that asked for the clone.
        self.current_origin = pending.op.scope().clone();
        self.last_created_op_id = None;
        let dispatched = self.apply(Command::DispatchCreateAgentRequest {
            request: Box::new(request),
            busy_message,
            term_size: (80, 24),
        });
        self.current_origin = crate::statusline::StatusScope::All;
        let created = self.last_created_op_id.take();
        let ending = match (created, dispatched) {
            (Some(create), Ok(reaction)) => {
                // The record following the clone follows the create from here,
                // holding the new project the way a create's own record does.
                self.operations.hold(
                    &op_id,
                    crate::operations::Hold::shared(InFlightKey::Project(project.id.clone())),
                );
                self.operations.hand_off(&op_id, &create);
                EventReaction::Multi(vec![
                    pending.op.resolve(&CloneOutcome::HandedOff).into_reaction(),
                    reaction,
                ])
            }
            (_, Ok(reaction)) => {
                let reason = crate::wire::wire_statuses_from_reaction(&reaction)
                    .into_iter()
                    .find(|status| status.tone == "error")
                    .map(|status| StatusText::from_parts(status.message, status.segments))
                    .unwrap_or_else(|| "the create did not start.".into());
                pending.op.resolve(&not_created(reason)).into_reaction()
            }
            (_, Err(error)) => pending
                .op
                .resolve(&not_created(format!("{error:#}").into()))
                .into_reaction(),
        };
        EventReaction::Multi(vec![EventReaction::RebuildLeftItems, ending])
    }
}
