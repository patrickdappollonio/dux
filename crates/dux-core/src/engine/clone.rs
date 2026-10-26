//! The engine's half of cloning a repository as a project with its first
//! agent: see [`crate::clone_project`] for the whole flow.

use std::path::PathBuf;
use std::time::Duration;

use super::{Command, Engine, EventReaction, InFlightKey, StatusUpdate};
use crate::clone_project::{
    CloneJob, CloneMode, CloneOutcome, CloneRequest, ClonedProject, ClonedRepository, PendingClone,
    PullRequestJob,
};
use crate::pr_reference::PullRequestCloneTarget;
use crate::status_text::StatusText;
use crate::worker::CreateAgentRequest;

/// Who asked for a clone, which decides who drives its follow-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloneAskedBy {
    /// A browser asked; the web layer drives the follow-up.
    Browser,
    /// Another surface asked; whoever drains the worker event drives it.
    OtherSurface,
}

impl Engine {
    /// Start a clone: run the checks that need neither git nor the
    /// destination, take the destination's in-flight key and hold, and spawn
    /// the worker. Answers the clone's keyed busy, or the sentence of the check
    /// that refused it, in which case nothing was started. `asked_by` says
    /// whether a browser asked, whose follow-up the web layer drives.
    pub(crate) fn begin_clone_project(
        &mut self,
        request: &CloneRequest,
        asked_by: CloneAskedBy,
    ) -> Result<StatusUpdate, StatusText> {
        let mode = if request.add_existing {
            CloneMode::AddExisting
        } else {
            CloneMode::Clone
        };
        if mode == CloneMode::AddExisting && request.pull_request.is_none() {
            return Err("add_existing needs a pull request.".into());
        }
        // For a pull request the address is built here from the policy and the
        // protocols as they are now, whatever the request carries.
        let offer = match &request.pull_request {
            Some(target) => Some(self.clone_offer_for(target)?),
            None => None,
        };
        let request = &match &offer {
            Some(offer) => CloneRequest {
                url: offer.address.clone(),
                ..request.clone()
            },
            None => request.clone(),
        };
        let projects: Vec<&str> = self.projects.iter().map(|p| p.path.as_str()).collect();
        let prepared =
            crate::clone_project::prepare_clone(request, home::home_dir().as_deref(), &projects)?;
        let destination = prepared.destination;
        let key = InFlightKey::Clone(destination.clone());
        if !self.mark_in_flight(key.clone()) {
            // The refusal says what the folder's holder is doing, whatever
            // this request would have done.
            return Err(
                if let Some(CloneMode::AddExisting) = self.clone_running_into(&destination) {
                    crate::status_text![
                        "dux is already adding ",
                        n(destination.display()),
                        " as a project. Wait for it to finish, or pick another folder."
                    ]
                } else {
                    crate::status_text![
                        "dux is already cloning a repository into ",
                        n(destination.display()),
                        " or setting one up there. Wait for it to finish, or pick another \
                 destination."
                    ]
                },
            );
        }
        // One clone per repository at a time, held until the project exists,
        // so two quick requests into different folders (a plain clone and a
        // pull request's included) cannot make two projects. An address that
        // names no host and repository has nothing to hold.
        let named = match &offer {
            Some(offer) => Some((offer.host.clone(), offer.owner_repo.clone())),
            None => crate::git::parse_remote_address(&prepared.url)
                .map(|remote| (remote.host, remote.owner_repo)),
        };
        let repository_key = named.map(|(host, owner_repo)| {
            format!("{}/{}", host.to_lowercase(), owner_repo.to_lowercase())
        });
        if let Some(repository) = &repository_key
            && !self.mark_in_flight(InFlightKey::CloneRepository(repository.clone()))
        {
            self.clear_in_flight(&key);
            let holder = self
                .clones
                .pending
                .values()
                .find(|held| held.repository_key.as_deref() == Some(repository.as_str()))
                .map(|held| held.mode);
            return Err(if let Some(CloneMode::AddExisting) = holder {
                crate::status_text![
                    "dux is already adding a checkout of ",
                    n(repository),
                    " for a new agent. Wait for it to finish, then use New agent from PR again."
                ]
            } else {
                crate::status_text![
                    "dux is already cloning ",
                    n(repository),
                    " for a new agent. Wait for it to finish, then use New agent from PR again."
                ]
            });
        }
        if let Err(refused) = self.hold_path_for_in_flight(
            &key,
            &destination,
            crate::worktree_ops::WorktreeOpKind::CloneRepository,
        ) {
            self.clear_in_flight(&key);
            self.release_repository_key(repository_key.as_deref());
            return Err(refused.sentence("clone a repository there"));
        }
        let address = crate::clone_project::redact_remote_userinfo(&prepared.url);
        let busy = if let (Some(offer), CloneMode::AddExisting) = (&offer, mode) {
            crate::status_text![
                "Adding ",
                n(destination.display()),
                " as a project for pull request ",
                n(format!("#{}", offer.number)),
                "..."
            ]
        } else {
            crate::status_text![
                "Cloning ",
                n(address),
                " into ",
                n(destination.display()),
                "..."
            ]
        };
        let op = crate::engine::status_op(busy)
            .resolve_in_handler(|outcome: &CloneOutcome| outcome.final_status())
            .with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        let pending = self.begin_status_op(&op);
        self.clones.pending.insert(
            op_id.clone(),
            PendingClone {
                op,
                from_web: asked_by == CloneAskedBy::Browser,
                repository_key: repository_key.clone(),
                mode,
                destination: destination.clone(),
                review_copy: request.review_copy.consent(),
            },
        );
        let job = CloneJob {
            status_op_id: op_id.clone(),
            url: prepared.url,
            destination: destination.clone(),
            agent_name: prepared.agent_name.clone(),
            stall: Duration::from_secs(self.config.git.clone_stall_seconds.max(1)),
            processes: self.clones.processes.clone(),
            pull_request: offer.as_ref().map(|offer| PullRequestJob {
                target: offer.target(),
                projects: self.projects.clone(),
                policy: self.github_host_policy(),
                gh_program: self.gh_probe.program.clone(),
                review_copy: request.review_copy.consent(),
                worktrees_root: self.paths.worktrees_root.clone(),
            }),
            repository_key: repository_key.clone(),
            mode,
            git_env: self.clones.git_env.clone(),
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
                let said = match mode {
                    CloneMode::Clone => StatusText::from(
                        "The clone worker stopped unexpectedly, so the clone may not have \
                         finished. Nothing was added to the workspace.",
                    ),
                    CloneMode::AddExisting => crate::status_text![
                        "The worker adding ",
                        n(destination.display()),
                        " as a project stopped unexpectedly. Nothing was added to the \
                         workspace, and the folder was not changed."
                    ],
                };
                let _ = worker_tx.send(crate::worker::WorkerEvent::RepositoryCloned {
                    status_op_id: Some(op_id),
                    path: destination,
                    agent_name,
                    repository_key,
                    mode,
                    result: Err(CloneOutcome::Failed(said)),
                });
            }
        });
        Ok(pending)
    }

    /// The offer a pull request clone is built from, from the engine's state
    /// as it is now: refused when GitHub integration is off, or when the
    /// target's host is not one `gh` is signed in to.
    fn clone_offer_for(
        &self,
        target: &PullRequestCloneTarget,
    ) -> Result<crate::pr_reference::PullRequestCloneOffer, StatusText> {
        if !self.pr_agent_command_available() {
            return Err(
                "GitHub PR agent creation requires GitHub integration and an authenticated gh CLI."
                    .into(),
            );
        }
        let reference = crate::pr_reference::TypedReference {
            host: Some(target.host.clone()),
            owner_repo: Some(target.owner_repo.clone()),
            number: Some(target.number),
        };
        crate::pr_reference::pull_request_clone_offer(
            &reference,
            &self.github_host_policy(),
            &self.github_git_protocols(),
        )
        .ok_or_else(|| {
            crate::status_text![
                "dux can't clone ",
                n(format!("{}/{}", target.host, target.owner_repo)),
                " because gh is not signed in to ",
                n(&target.host),
                ". Sign in with gh, then use New agent from PR again."
            ]
        })
    }

    /// Let go of the repository a pull request clone held. Releasing a key
    /// nobody holds does nothing, so every path that might own it calls this.
    fn release_repository_key(&mut self, key: Option<&str>) {
        if let Some(key) = key {
            self.clear_in_flight(&InFlightKey::CloneRepository(key.to_string()));
        }
    }

    /// What is being done to `path` by a clone or an add that holds it, so a
    /// request to set a repository up there can say what it waits for: the
    /// mode of the pending operation for that folder, or a clone when the key
    /// is held and no operation names it.
    pub(crate) fn clone_running_into(&self, path: &std::path::Path) -> Option<CloneMode> {
        self.is_in_flight(&InFlightKey::Clone(path.to_path_buf()))
            .then(|| {
                self.clones
                    .pending
                    .values()
                    .find(|held| held.destination == path)
                    .map_or(CloneMode::Clone, |held| held.mode)
            })
    }

    /// The refusal for a request to set a repository up at `path` while a clone
    /// or an add holds it, worded by what the holder is doing.
    pub(crate) fn clone_running_refusal(&self, path: &std::path::Path) -> Option<String> {
        Some(match self.clone_running_into(path)? {
            CloneMode::Clone => format!(
                "dux is cloning a repository into \"{}\". Please wait for it to finish.",
                path.display()
            ),
            CloneMode::AddExisting => format!(
                "dux is adding \"{}\" as a project for a pull request. Please wait for it to \
                 finish.",
                path.display()
            ),
        })
    }

    /// A clone's worker finished: free the destination, and end the operation
    /// when the clone stopped short of a project, or hand a ready clone to the
    /// surface that adds it.
    pub(crate) fn process_repository_cloned(
        &mut self,
        status_op_id: Option<String>,
        path: PathBuf,
        agent_name: String,
        repository_key: Option<String>,
        mode: CloneMode,
        result: Result<ClonedRepository, CloneOutcome>,
    ) -> EventReaction {
        self.clear_in_flight(&InFlightKey::Clone(path.clone()));
        match result {
            // The repository stays held until the project is registered.
            Ok(cloned) => EventReaction::AddProjectAfterClone(Box::new(ClonedProject {
                status_op_id,
                path,
                agent_name,
                cloned,
                repository_key,
                mode,
            })),
            Err(outcome) => {
                // No project follows this answer, so the repository is free.
                self.release_repository_key(repository_key.as_deref());
                match status_op_id
                    .as_ref()
                    .and_then(|id| self.clones.pending.remove(id))
                {
                    Some(pending) => {
                        self.release_repository_key(pending.repository_key.as_deref());
                        pending.op.resolve(&outcome).into_reaction()
                    }
                    // The operation is gone, but its busy may still be on a
                    // screen: the final carries its key so it replaces it.
                    None => {
                        let keyed = |update: StatusUpdate| match &status_op_id {
                            Some(id) => update.with_key(id.clone()),
                            None => update,
                        };
                        match outcome {
                            CloneOutcome::Refused(text) | CloneOutcome::Failed(text) => {
                                EventReaction::Status(keyed(StatusUpdate::error(text)))
                            }
                            CloneOutcome::Kept(text) => {
                                EventReaction::Status(keyed(StatusUpdate::warning(text).sticky()))
                            }
                            CloneOutcome::HandedOff => EventReaction::Nothing,
                        }
                    }
                }
            }
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
            self.release_repository_key(done.repository_key.as_deref());
            // Nothing follows, so the clone's busy is retired here.
            return match &done.status_op_id {
                Some(id) => EventReaction::ClearStatus(id.clone()),
                None => EventReaction::Nothing,
            };
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
        // The project exists or never will: either way the repository is free.
        self.release_repository_key(done.repository_key.as_deref());
        self.release_repository_key(pending.repository_key.as_deref());
        let Some(project) = added
            .added
            .as_ref()
            .and_then(|(id, _)| self.projects.iter().find(|p| &p.id == id).cloned())
        else {
            let reason = added.failure(&"the add did not say why.".into());
            let kept = match done.mode {
                CloneMode::Clone => crate::status_text![
                    "Cloned ",
                    n(address),
                    " into ",
                    n(path),
                    ", but couldn't add it as a project: ",
                    reason,
                    " The clone is still there, and no agent was created."
                ],
                CloneMode::AddExisting => crate::status_text![
                    "Couldn't add ",
                    n(path),
                    " as a project: ",
                    reason,
                    " The folder is untouched, and no agent was created."
                ],
            };
            return pending
                .op
                .resolve(&CloneOutcome::Kept(kept))
                .into_reaction();
        };
        let agent = &done.agent_name;
        let cloned = crate::clone_project::ClonedFor {
            address: address.clone(),
            path: done.path.clone(),
            project_name: project.name.clone(),
            project_id: project.id.clone(),
            agent_name: agent.clone(),
            mode: done.mode,
            pull_request: done
                .cloned
                .pull_request
                .as_ref()
                .map(|pull_request| pull_request.target.clone()),
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
        // A pull request's agent starts on the pull request's own code, under
        // the rules every pull request create follows: the create decides
        // about a branch of that name and about a review copy, which this
        // request does not agree to.
        let (request, busy_message) = match &done.cloned.pull_request {
            Some(pull_request) => (
                CreateAgentRequest::PullRequest {
                    project: project.clone(),
                    host: pull_request.target.host.clone(),
                    owner_repo: pull_request.target.owner_repo.clone(),
                    number: pull_request.target.number,
                    title: pull_request.title.clone(),
                    state: pull_request.state.clone(),
                    head_branch: pull_request.head_branch.clone(),
                    custom_name: Some(agent.clone()),
                    use_existing_branch: false,
                    review_copy: pending.review_copy.clone(),
                    ask_id: None,
                    agents: Vec::new(),
                    cloned: Some(cloned.clone()),
                },
                crate::status_text![
                    "Creating a new agent worktree ",
                    q(agent),
                    " from PR ",
                    n(format!("#{}", pull_request.target.number)),
                    " for project ",
                    q(project.name),
                    " and launching a fresh session..."
                ],
            ),
            None => (
                CreateAgentRequest::NewProject {
                    project: project.clone(),
                    custom_name: Some(agent.clone()),
                    use_existing_branch: false,
                    pull_before_create: false,
                    copy_uncommitted_changes: false,
                    cloned: Some(cloned.clone()),
                },
                crate::status_text![
                    "Creating agent ",
                    q(agent),
                    " in project ",
                    q(project.name),
                    " and launching a fresh session..."
                ],
            ),
        };
        // The create mints its busy from `current_origin`, so it reaches the
        // connection that asked for the clone.
        let scope = pending.op.scope().clone();
        self.current_origin = scope.clone();
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
                    &crate::operations::Hold::shared(InFlightKey::Project(project.id.clone())),
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
                    .map_or_else(
                        || "the create did not start.".into(),
                        |status| StatusText::from_parts(status.message, status.segments),
                    );
                pending.op.resolve(&not_created(reason)).into_reaction()
            }
            (_, Err(error)) => pending
                .op
                .resolve(&not_created(format!("{error:#}").into()))
                .into_reaction(),
        };
        let mut reactions = vec![EventReaction::RebuildLeftItems, ending];
        // Only after the project exists: a guess about a project that was not
        // added is no use to anyone.
        if let (Some(guess), Some(pull_request)) =
            (&done.cloned.leading_branch_guess, &done.cloned.pull_request)
        {
            let repository = format!(
                "{}/{}",
                pull_request.target.host, pull_request.target.owner_repo
            );
            let why = match &guess.remote_default {
                None => crate::status_text![
                    "dux could not learn the default branch of ",
                    n(repository),
                    " from its remote, so project "
                ],
                Some(default) => crate::status_text![
                    "dux learned that the default branch of ",
                    n(repository),
                    " is ",
                    q(default),
                    ", but the folder has no local branch of that name, so project "
                ],
            };
            let tail = if guess.checked_out {
                ", the branch the folder has checked out."
            } else {
                ", because the folder has no branch checked out."
            };
            reactions.push(EventReaction::Status(
                StatusUpdate::warning(crate::status_text![
                    why,
                    q(&project.name),
                    " leads with ",
                    q(&done.cloned.leading_branch),
                    tail
                ])
                .with_scope(scope),
            ));
        }
        EventReaction::Multi(reactions)
    }
}
