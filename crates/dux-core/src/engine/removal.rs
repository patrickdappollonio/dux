//! The engine's half of removal coordination: the per-worktree registry of
//! operations in flight ([`crate::worktree_ops`]), the removals announced on it,
//! and the project-delete cascade that routes every agent through the same
//! deferred, worker-run removal a single delete takes.
//!
//! The seam into process lifecycle is deliberately one call: a cascade begins
//! each agent's delete through [`Engine::begin_delete_session`], which stops the
//! agent's processes and parks the worktree removal until they have exited
//! (`reap_terminating_ptys` hands it to
//! [`Engine::dispatch_deferred_worktree_removal`]). Nothing here waits for a
//! process itself.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use crate::engine::{
    BeginDeleteSessionOutcome, Engine, EventReaction, Final, HandlerStatusOp, InFlightKey,
    StatusUpdate,
};
use crate::status_text::StatusText;
use crate::worker::WorkerEvent;
use crate::worktree_ops::{HoldOwner, HoldRefused, RemovalClaim, WorktreeOpKind, WorktreeOps};

/// Everything the engine keeps about removals in flight. One field on the
/// engine so its constructors do not each have to know the parts.
#[derive(Default)]
pub struct RemovalCoordination {
    /// The registry itself, shared by handle with workers and the web routes.
    pub ops: WorktreeOps,
    /// A worktree-removing delete announces its removal the moment it begins,
    /// keyed by session id, so the folder is "being removed" through the whole
    /// grace period; the claim moves into the removal worker when it runs.
    pub(crate) claims: HashMap<String, RemovalClaim>,
    /// Project deletions in progress, by project id.
    pub(crate) project_deletions: HashMap<String, ProjectDeletion>,
    /// Session id to the project whose cascade is waiting on its removal.
    pub(crate) cascade_members: HashMap<String, String>,
    /// Agents being created, by create op id, and the project each is for.
    pub(crate) creating: HashMap<String, Option<String>>,
    /// The agent's label by session id, kept while its removal runs so a
    /// waiting status can name an agent whose record is already gone.
    pub(crate) labels: HashMap<String, String>,
}

/// What a startup-command rerun holds while it runs: the agent's one run, and
/// its hold on the worktree. Both are released when this drops.
pub struct StartupRerunClaim {
    pub run: crate::process_sessions::StartupRunGuard,
    pub hold: crate::worktree_ops::WorktreeOpGuard,
}

/// What a project deletion reports when its last worktree removal lands.
pub struct ProjectDeletionOutcome {
    pub project_name: String,
    pub agents: usize,
    /// Each worktree that could not be removed, and why.
    pub failures: Vec<(String, String)>,
    /// What could not be saved, and why, when something could not: the
    /// session database or config.toml.
    pub persist_failure: Option<(String, String)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeletionPhase {
    /// Waiting, bounded, for an agent being created in the project to finish.
    AwaitingCreate,
    /// The records are gone and the worktree removals are running.
    Removing,
}

pub(crate) struct ProjectDeletion {
    project_name: String,
    was_real: bool,
    phase: DeletionPhase,
    op: Option<HandlerStatusOp<ProjectDeletionOutcome>>,
    pending: HashSet<String>,
    /// Worktree path by session id, so a failure can name the folder.
    paths: HashMap<String, String>,
    agents: usize,
    failures: Vec<(String, String)>,
    persist_failure: Option<(String, String)>,
}

/// " and its agent" / " and its 3 agents" / "".
pub(crate) fn removed_agents_detail(removed: usize) -> String {
    match removed {
        0 => String::new(),
        1 => " and its agent".to_string(),
        count => format!(" and its {count} agents"),
    }
}

/// The final a project deletion ends on.
pub fn project_deletion_final(outcome: &ProjectDeletionOutcome) -> Final {
    let detail = removed_agents_detail(outcome.agents);
    if let Some((what, error)) = &outcome.persist_failure {
        return Final::error(crate::status_text![
            "Deleted ",
            q(outcome.project_name.clone()),
            format!(
                "{detail} from dux, but updating {what} failed: {error}. The project \
                 may reappear on restart. Check the file is writable."
            )
        ]);
    }
    if outcome.failures.is_empty() {
        return Final::info(crate::status_text![
            "Deleted project ",
            q(outcome.project_name.clone()),
            format!("{detail}. Worktrees were removed.")
        ]);
    }
    let mut text = crate::status_text![
        "Deleted project ",
        q(outcome.project_name.clone()),
        format!(
            "{detail}, but {} could not be removed and {} still on disk: ",
            crate::model::count_words(outcome.failures.len(), "worktree", "worktrees"),
            if outcome.failures.len() == 1 {
                "is"
            } else {
                "are"
            }
        )
    ];
    for (index, (path, reason)) in outcome.failures.iter().enumerate() {
        let separator = if index == 0 { "" } else { "; " };
        text = crate::status_text![
            text,
            separator,
            n(path.clone()),
            format!(" ({})", reason.trim_end_matches('.'))
        ];
    }
    // Sticky: folders were left behind that the user has to remove by hand.
    Final::warning(crate::status_text![
        text,
        ". Delete each folder yourself once nothing is using it."
    ])
    .sticky()
}

/// The status a removal shows while it waits for operations in its worktree.
pub fn removal_waiting_message(label: &str, waiting_for: &str) -> StatusText {
    crate::status_text![
        "Removing worktree for agent ",
        q(label.to_string()),
        format!(": waiting for {waiting_for} to finish first\u{2026}")
    ]
}

/// The failure a removal reports when the operations in its worktree did not
/// finish within the wait.
pub fn removal_wait_expired_message(path: &Path, waited: Duration, still: &str) -> String {
    format!(
        "dux waited {} seconds for {still} in {} to finish and it had not, so the worktree \
         was kept. Remove it from the worktree manager once that is done, or raise \
         removal_wait_seconds in config.toml.",
        waited.as_secs(),
        crate::home_path::shorten_home(path)
    )
}

impl Engine {
    /// The per-worktree registry, for surfaces and workers that hold paths.
    pub fn worktree_ops(&self) -> &WorktreeOps {
        &self.removal_coordination.ops
    }

    /// Claim everything a startup-command rerun needs before it starts, or
    /// the ONE sentence that says why it may not, decided here for both
    /// surfaces:
    ///
    /// - the agent's one run at a time (its process registry): a second rerun
    ///   while the first is going is refused;
    /// - a hold on the worktree, so a removal waits for the run, and a
    ///   worktree already being removed is not provisioned again.
    ///
    /// The two refusals cannot both apply to one request: the first is taken
    /// before the hold is asked for, and a refused hold gives the run claim
    /// back on the spot.
    pub fn claim_startup_rerun(
        &self,
        session_id: &str,
        agent_label: &str,
        worktree_path: &str,
    ) -> Result<StartupRerunClaim, StatusText> {
        let Some(run) = self.process_registry.begin_startup_run(session_id) else {
            return Err(crate::startup::startup_already_running_message(agent_label).into());
        };
        let hold = self
            .worktree_ops()
            .hold(worktree_path, WorktreeOpKind::StartupCommand)
            .map_err(|refused| refused.sentence("rerun its startup command"))?;
        Ok(StartupRerunClaim { run, hold })
    }

    /// How long a removal waits for operations in its worktree (and a project
    /// deletion for an agent being created in it) before it gives up out loud.
    pub fn removal_wait(&self) -> Duration {
        Duration::from_secs(u64::from(self.config.removal_wait_seconds))
    }

    /// Register `path` for the operation behind the in-flight `key`. Released
    /// by [`Engine::clear_in_flight`] for that key, so every completion path
    /// that already clears the key releases the path too.
    pub fn hold_path_for_in_flight(
        &self,
        key: &InFlightKey,
        path: impl AsRef<Path>,
        kind: WorktreeOpKind,
    ) -> Result<(), HoldRefused> {
        self.removal_coordination
            .ops
            .hold_as(HoldOwner::InFlight(key.clone()), path, kind)
    }

    /// Record an agent create starting, for the project-delete wait.
    pub(crate) fn note_create_started(&mut self, op_id: &str, project_id: Option<&str>) {
        self.removal_coordination
            .creating
            .insert(op_id.to_string(), project_id.map(str::to_string));
    }

    /// Record an agent create ending, however it ended: release the path the
    /// create held, and wake a project deletion that was waiting for it.
    pub(crate) fn note_create_finished(&mut self, op_id: &str) {
        self.removal_coordination
            .ops
            .release_owner(&HoldOwner::CreateOp(op_id.to_string()));
        let Some(Some(project_id)) = self.removal_coordination.creating.remove(op_id) else {
            return;
        };
        if self
            .removal_coordination
            .project_deletions
            .get(&project_id)
            .is_some_and(|deletion| deletion.phase == DeletionPhase::AwaitingCreate)
        {
            let _ = self
                .worker_tx
                .send(WorkerEvent::ProjectDeletionContinue { project_id });
        }
    }

    /// Whether an agent is being created in this project right now.
    pub(crate) fn project_has_create_in_flight(&self, project_id: &str) -> bool {
        self.removal_coordination
            .creating
            .values()
            .any(|project| project.as_deref() == Some(project_id))
    }

    /// Whether this project is being deleted.
    pub fn project_is_being_deleted(&self, project_id: &str) -> bool {
        self.removal_coordination
            .project_deletions
            .contains_key(project_id)
    }

    /// Announce the removal of a deleted agent's worktree now, at the start of
    /// its delete, so nothing new starts in the folder through the grace
    /// period. The claim travels to the removal worker.
    pub(crate) fn announce_session_removal(
        &mut self,
        session_id: &str,
        worktree_path: &str,
        label: String,
    ) {
        let claim = self
            .removal_coordination
            .ops
            .announce_removal(worktree_path);
        self.removal_coordination
            .claims
            .insert(session_id.to_string(), claim);
        self.removal_coordination
            .labels
            .insert(session_id.to_string(), label);
    }

    /// `Command::DeleteProject`: delete a project, every agent in it and their
    /// worktrees. The records go at once; each worktree goes through the same
    /// deferred, worker-run removal a single agent delete takes, so it waits
    /// for that agent's processes and for any operation in flight in it. One
    /// keyed busy covers the whole cascade and one final names every worktree
    /// that could not be removed.
    pub(crate) fn delete_project_with_worktrees(
        &mut self,
        project_id: &str,
        project_name: &str,
    ) -> anyhow::Result<EventReaction> {
        if self.project_is_being_deleted(project_id) {
            return Ok(EventReaction::Status(StatusUpdate::warning(
                crate::status_text![
                    "Project ",
                    q(project_name),
                    " is already being deleted. The status line says when it is done."
                ],
            )));
        }
        if self.project_has_launching_tab(project_id) {
            return Ok(EventReaction::Status(StatusUpdate::error(
                crate::status_text![
                    "Cannot delete project ",
                    q(project_name),
                    " while an agent tab is still launching. \
                 Wait a moment, then try again."
                ],
            )));
        }
        let awaiting_create = self.project_has_create_in_flight(project_id);
        let busy = if awaiting_create {
            crate::status_text![
                "Deleting project ",
                q(project_name),
                ": waiting for the agent being created in it to finish first\u{2026}"
            ]
        } else {
            crate::status_text![
                "Deleting project ",
                q(project_name),
                " and removing its worktrees\u{2026}"
            ]
        };
        let op = crate::engine::status_op(busy)
            .resolve_in_handler(project_deletion_final)
            .with_scope(self.current_origin.clone());
        let pending = self.begin_status_op(&op);
        // No new tab may start in an agent that is about to be deleted.
        for session in &self.sessions {
            if session.project_id() == Some(project_id) {
                self.closing_sessions.insert(session.id.clone());
            }
        }
        self.removal_coordination.project_deletions.insert(
            project_id.to_string(),
            ProjectDeletion {
                project_name: project_name.to_string(),
                was_real: self.projects.iter().any(|project| project.id == project_id),
                phase: DeletionPhase::AwaitingCreate,
                op: Some(op),
                pending: HashSet::new(),
                paths: HashMap::new(),
                agents: 0,
                failures: Vec::new(),
                persist_failure: None,
            },
        );
        if awaiting_create {
            // Bounded: a create that outlasts the wait is caught by the
            // launch-ready defence for a project that no longer exists.
            let tx = self.worker_tx.clone();
            let wait = self.removal_wait();
            let project_id = project_id.to_string();
            std::thread::spawn(move || {
                std::thread::sleep(wait);
                let _ = tx.send(WorkerEvent::ProjectDeletionContinue { project_id });
            });
            return Ok(EventReaction::Status(pending));
        }
        let mut reactions = vec![EventReaction::Status(pending)];
        reactions.extend(self.proceed_project_deletion(project_id));
        Ok(EventReaction::Multi(reactions))
    }

    /// `WorkerEvent::ProjectDeletionContinue`: the create a deletion waited for
    /// finished, or the wait ran out. Idempotent: only the first one proceeds.
    pub(crate) fn process_project_deletion_continue(&mut self, project_id: &str) -> EventReaction {
        EventReaction::Multi(self.proceed_project_deletion(project_id))
    }

    fn proceed_project_deletion(&mut self, project_id: &str) -> Vec<EventReaction> {
        match self
            .removal_coordination
            .project_deletions
            .get_mut(project_id)
        {
            Some(deletion) if deletion.phase == DeletionPhase::AwaitingCreate => {
                deletion.phase = DeletionPhase::Removing;
            }
            _ => return Vec::new(),
        }
        let session_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|session| session.project_id() == Some(project_id))
            .map(|session| session.id.clone())
            .collect();
        let mut pending = HashSet::new();
        let mut paths = HashMap::new();
        for session_id in &session_ids {
            let path = self
                .sessions
                .iter()
                .find(|session| &session.id == session_id)
                .map(|session| session.directory().to_string())
                .unwrap_or_default();
            // `None` for the branch: the project-removal dialog asks about the
            // project rather than each agent's branch, so the provenance default
            // stands. That is what keeps a project removal from taking a user's
            // `develop` with it.
            match self.begin_delete_session(session_id, true, None) {
                BeginDeleteSessionOutcome::AsyncStarted { .. } => {
                    pending.insert(session_id.clone());
                    paths.insert(session_id.clone(), path);
                    self.removal_coordination
                        .cascade_members
                        .insert(session_id.clone(), project_id.to_string());
                }
                BeginDeleteSessionOutcome::Inline { .. } | BeginDeleteSessionOutcome::NotFound => {}
                other => crate::logger::warn(&format!(
                    "project delete {project_id}: agent {session_id} was not routed through \
                     a worktree removal ({other:?}); its record goes with the project"
                )),
            }
            // Out of memory now, before the next agent is asked about: two
            // agents sharing one worktree each see the other as a sibling until
            // it is gone, and then neither would remove it. The rows go with
            // the project's records below, in one transaction.
            self.finish_delete_session_memory(session_id);
            self.closing_sessions.remove(session_id);
        }
        let records_failure = self
            .session_store
            .remove_project_records(project_id)
            .err()
            .map(|error| ("the session database".to_string(), format!("{error:#}")));
        self.remove_project_from_runtime(project_id);
        let was_real = self
            .removal_coordination
            .project_deletions
            .get(project_id)
            .is_some_and(|deletion| deletion.was_real);
        let config_failure = if was_real {
            self.persist_projects_to_config()
                .err()
                .map(|error| ("config.toml".to_string(), format!("{error:#}")))
        } else {
            None
        };
        if let Some(deletion) = self
            .removal_coordination
            .project_deletions
            .get_mut(project_id)
        {
            deletion.pending = pending;
            deletion.paths = paths;
            deletion.agents = session_ids.len();
            deletion.persist_failure = records_failure.or(config_failure);
        }
        let mut reactions = vec![EventReaction::RebuildLeftItems];
        reactions.extend(self.finish_project_deletion_if_done(project_id));
        reactions
    }

    fn finish_project_deletion_if_done(&mut self, project_id: &str) -> Option<EventReaction> {
        let done = self
            .removal_coordination
            .project_deletions
            .get(project_id)
            .is_some_and(|deletion| {
                deletion.phase == DeletionPhase::Removing && deletion.pending.is_empty()
            });
        if !done {
            return None;
        }
        let mut deletion = self
            .removal_coordination
            .project_deletions
            .remove(project_id)?;
        let outcome = ProjectDeletionOutcome {
            project_name: deletion.project_name.clone(),
            agents: deletion.agents,
            failures: std::mem::take(&mut deletion.failures),
            persist_failure: deletion.persist_failure.take(),
        };
        deletion
            .op
            .take()
            .map(|op| op.resolve(&outcome).into_reaction())
    }

    /// A cascade member's worktree removal finished. Answers `None` when the
    /// session is not part of a project deletion, so the ordinary single-delete
    /// report runs instead.
    pub(crate) fn cascade_removal_completed(
        &mut self,
        session_id: &str,
        failure: Option<&str>,
    ) -> Option<EventReaction> {
        let project_id = self
            .removal_coordination
            .cascade_members
            .remove(session_id)?;
        if let Some(deletion) = self
            .removal_coordination
            .project_deletions
            .get_mut(&project_id)
        {
            deletion.pending.remove(session_id);
            if let Some(reason) = failure {
                let path = deletion
                    .paths
                    .get(session_id)
                    .map(|path| crate::home_path::shorten_home(Path::new(path)))
                    .unwrap_or_else(|| session_id.to_string());
                deletion.failures.push((path, reason.to_string()));
            }
        }
        Some(
            self.finish_project_deletion_if_done(&project_id)
                .unwrap_or(EventReaction::Nothing),
        )
    }

    /// A cascade member's removal is waiting for operations in its worktree.
    pub(crate) fn cascade_removal_waiting(
        &self,
        session_id: &str,
        waiting_for: &str,
    ) -> Option<EventReaction> {
        let project_id = self.removal_coordination.cascade_members.get(session_id)?;
        let deletion = self
            .removal_coordination
            .project_deletions
            .get(project_id)?;
        let path = deletion
            .paths
            .get(session_id)
            .map(|path| crate::home_path::shorten_home(Path::new(path)))
            .unwrap_or_default();
        let op = deletion.op.as_ref()?;
        Some(EventReaction::Status(op.progress(crate::status_text![
            "Deleting project ",
            q(deletion.project_name.clone()),
            ": waiting for ",
            waiting_for.to_string(),
            " in ",
            n(path),
            " to finish before removing it\u{2026}"
        ])))
    }

    /// An agent finished launching for a project that was deleted while it was
    /// being created. Stop it through the same deferred path a delete takes:
    /// SIGTERM now, and once it has exited remove the worktree, but only when
    /// this launch made it, and its branch only when dux minted it (the
    /// provenance default, with nobody asked). Answers the sentence the
    /// create's final says.
    pub(crate) fn stop_create_for_deleted_project(
        &mut self,
        session: &crate::model::AgentSession,
        tab_id: &crate::ids::TabId,
        client: crate::pty::PtyClient,
        repo_path: &str,
        owns_worktree: bool,
    ) -> StatusText {
        let label = session.display_label();
        let removal = match session.workspace.as_managed() {
            Some(managed) if owns_worktree => {
                self.announce_session_removal(&session.id, &managed.worktree_path, label.clone());
                Some(super::DeferredWorktreeRemoval {
                    session_id: session.id.clone(),
                    project_path: repo_path.to_string(),
                    managed: managed.clone(),
                    delete_branch: None,
                    busy_message: crate::status_text![
                        "Removing worktree for agent ",
                        q(label.clone()),
                        "\u{2026}"
                    ],
                    processes: super::RemovalProcesses::none(),
                })
            }
            _ => None,
        };
        let folder = crate::home_path::shorten_home(Path::new(session.directory()));
        let branch_note = match session.workspace.as_managed() {
            Some(managed) if owns_worktree && managed.branch_provenance.dux_may_delete_branch() => {
                crate::status_text![
                    " and the branch ",
                    q(managed.branch_name.clone()),
                    " it minted"
                ]
            }
            Some(managed) if owns_worktree => crate::status_text![
                "; the branch ",
                q(managed.branch_name.clone()),
                " already existed, so it is kept"
            ],
            _ => crate::status_text![""],
        };
        crate::logger::warn(&format!(
            "agent {} finished launching after its project was deleted; stopping it",
            session.id
        ));
        // The process joins the providers map only to be handed straight to the
        // deferred close, which takes it out again. With a worktree to remove it
        // goes through the same process-session wait a delete takes.
        let process = client.process_session();
        self.providers.insert(tab_id.clone(), client);
        let owner = super::TerminatingOwner {
            session_id: session.id.clone(),
            provider: Some(session.provider.clone()),
        };
        match removal {
            Some(mut removal) => {
                let targets: Vec<_> = self
                    .move_provider_to_terminating(tab_id, label.clone(), Some(owner))
                    .into_iter()
                    .collect();
                let pending_ids = if targets.is_empty() {
                    HashSet::new()
                } else {
                    HashSet::from([tab_id.as_str().to_string()])
                };
                removal.processes = self.removal_processes_for(process.into_iter().collect());
                self.record_and_park_removal(&label, removal, targets, pending_ids);
            }
            None => {
                self.begin_close_provider(tab_id, label.clone(), Some(owner));
            }
        }
        if owns_worktree {
            crate::status_text![
                "Agent ",
                q(label),
                " finished starting after its project was deleted, so dux stopped it and is \
                 removing the worktree it made at ",
                n(folder),
                branch_note,
                "."
            ]
        } else {
            crate::status_text![
                "Agent ",
                q(label),
                " finished starting after its project was deleted, so dux stopped it. Its \
                 worktree at ",
                n(folder),
                " existed before this agent, so it was left in place."
            ]
        }
    }

    /// Decide a worktree-manager removal against LIVE state, at the moment it
    /// is asked for, never against the listing the user was looking at: an
    /// agent holding the folder now, one being created on it now, or a removal
    /// of it already under way (its agent was just deleted) refuses it. An
    /// admitted removal is announced before this returns, so from here on
    /// nothing new starts in the folder. `None` for an unknown project.
    pub fn admit_manager_removal(
        &self,
        project_id: &str,
        requested: &Path,
        delete_branch: bool,
    ) -> Option<crate::worktree_manager::RemovalAdmission> {
        use crate::worktree_manager::{AdmittedRemoval, RemovalAdmission, RemovalOutcome};
        let project = self
            .projects
            .iter()
            .find(|project| project.id == project_id)?
            .clone();
        let requested_text = requested.to_string_lossy();
        let attached = self.sessions.iter().any(|session| {
            crate::project_browser::same_directory(session.directory(), &requested_text)
        }) || self
            .removal_coordination
            .ops
            .holders(requested)
            .contains(&WorktreeOpKind::CreateAgent);
        if attached {
            return Some(RemovalAdmission::Refused(RemovalOutcome::Attached));
        }
        // Something dux started still runs in the folder though no listed agent
        // holds it: a deleted agent's CLI still stopping, a terminal. Refused
        // and named, never removed under it.
        if let Some(reason) = self.folder_busy_reason(requested) {
            return Some(RemovalAdmission::Refused(RemovalOutcome::Busy { reason }));
        }
        match self.removal_coordination.ops.announce_removal(requested) {
            RemovalClaim::Join(_) => Some(RemovalAdmission::Refused(RemovalOutcome::BeingRemoved)),
            RemovalClaim::Lead(lease) => {
                Some(RemovalAdmission::Admitted(Box::new(AdmittedRemoval {
                    lease,
                    project,
                    paths: self.paths.clone(),
                    sessions: self.sessions.clone(),
                    requested: requested.to_path_buf(),
                    delete_branch,
                    wait: self.removal_wait(),
                    processes: super::RemovalProcesses {
                        sessions: self.process_sessions_in(requested),
                        grace: self.individual_close_grace(),
                        ..super::RemovalProcesses::none()
                    },
                    registry: self.process_registry.clone(),
                })))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::engine::{Command, RemovedBranches};
    use crate::statusline::StatusTone;
    use crate::worker::WorkerEvent;
    use crate::worktree_ops::WorktreeOpKind;

    fn git(dir: &Path, args: &[&str]) {
        let out = crate::test_git::fixture_git()
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn branch_exists(repo: &Path, branch: &str) -> bool {
        crate::test_git::fixture_git()
            .args([
                "-C",
                repo.to_str().unwrap(),
                "show-ref",
                "--verify",
                "--quiet",
            ])
            .arg(format!("refs/heads/{branch}"))
            .status()
            .unwrap()
            .success()
    }

    /// A repository registered as project `p1`, and one dux-made worktree per
    /// name, each on its own dux-minted branch, each an agent `s-<name>`.
    fn world(engine: &mut Engine, root: &Path, names: &[&str]) -> (PathBuf, Vec<PathBuf>) {
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("f.txt"), "hi").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        engine
            .projects
            .push(sample_project("p1", repo.to_str().unwrap()));
        let mut worktrees = Vec::new();
        for name in names {
            // Under dux's worktrees root for this project, as dux makes them.
            let worktree = root.join("worktrees").join("p1-name").join(name);
            std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
            git(
                &repo,
                &["worktree", "add", "-b", name, worktree.to_str().unwrap()],
            );
            let mut session = sample_session(&format!("s-{name}"), "p1", name);
            if let Some(managed) = session.workspace.as_managed_mut() {
                managed.worktree_path = worktree.to_string_lossy().into_owned();
            }
            engine.sessions.push(session);
            worktrees.push(worktree);
        }
        (repo, worktrees)
    }

    /// Process worker events until one matches, and answer that event's
    /// reaction. Everything before it is processed too, as a surface would.
    fn pump_until(engine: &mut Engine, matches: impl FnMut(&WorkerEvent) -> bool) -> EventReaction {
        pump_collect(engine, matches).pop().unwrap()
    }

    /// The same, answering every reaction processed on the way, the matching
    /// one last.
    fn pump_collect(
        engine: &mut Engine,
        mut matches: impl FnMut(&WorkerEvent) -> bool,
    ) -> Vec<EventReaction> {
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(200)) else {
                continue;
            };
            let hit = matches(&event);
            seen.push(engine.process_worker_event(event));
            if hit {
                return seen;
            }
        }
        panic!("the expected worker event never arrived");
    }

    fn statuses(reaction: &EventReaction) -> Vec<StatusUpdate> {
        match reaction {
            EventReaction::Status(status) => vec![status.clone()],
            EventReaction::Multi(all) => all.iter().flat_map(statuses).collect(),
            _ => Vec::new(),
        }
    }

    fn is_completion(event: &WorkerEvent) -> bool {
        matches!(event, WorkerEvent::WorktreeRemoveCompleted { .. })
    }

    fn is_waiting(event: &WorkerEvent) -> bool {
        matches!(event, WorkerEvent::WorktreeRemoveWaiting { .. })
    }

    /// Item 10: a delete that finds a pull running in the worktree waits for it,
    /// says so, and removes the worktree once the pull's completion lands.
    #[test]
    fn a_removal_waits_for_a_pull_running_in_the_worktree() {
        let (mut engine, tmp) = test_engine();
        let (repo, worktrees) = world(&mut engine, tmp.path(), &["agent"]);
        let worktree = &worktrees[0];
        let pull = InFlightKey::Pull(worktree.to_string_lossy().into_owned());
        engine.mark_in_flight(pull.clone());
        engine
            .hold_path_for_in_flight(&pull, worktree, WorktreeOpKind::Pull)
            .unwrap();

        let outcome = engine.begin_delete_session("s-agent", true, None);
        assert!(matches!(
            outcome,
            BeginDeleteSessionOutcome::AsyncStarted { .. }
        ));
        let waiting = pump_until(&mut engine, is_waiting);
        let EventReaction::WorktreeRemoveWaiting { message, .. } = waiting else {
            panic!("a waiting removal tells its surface what it waits for");
        };
        assert!(
            message.to_string().contains("waiting for a pull"),
            "{message}"
        );
        assert!(worktree.exists(), "nothing is removed while the pull runs");
        assert!(
            engine
                .worker_rx
                .recv_timeout(Duration::from_millis(500))
                .is_err(),
            "the removal must not finish while the pull holds the worktree"
        );
        assert!(worktree.exists(), "nothing is removed while the pull runs");

        engine.clear_in_flight(&pull);
        let done = pump_until(&mut engine, is_completion);
        assert!(matches!(
            done,
            EventReaction::WorktreeRemoveSucceeded { .. }
        ));
        assert!(!worktree.exists());
        assert!(!branch_exists(&repo, "agent"));
    }

    /// Item 10: a rename that was running when the delete began moves the
    /// branch; the removal deletes it by its NEW name rather than leaving it
    /// behind and deleting a name that is already gone.
    #[test]
    fn a_removal_deletes_the_branch_by_the_name_a_running_rename_gave_it() {
        let (mut engine, tmp) = test_engine();
        let (repo, worktrees) = world(&mut engine, tmp.path(), &["old"]);
        let worktree = worktrees[0].clone();
        let crate::engine::BranchRenamePlan::RenameBranch(dispatch) =
            engine.prepare_branch_rename("s-old", "new", true)
        else {
            panic!("a real rename is planned");
        };
        engine.mark_in_flight(InFlightKey::BranchRename("s-old".into()));

        assert!(matches!(
            engine.begin_delete_session("s-old", true, None),
            BeginDeleteSessionOutcome::AsyncStarted { .. }
        ));
        pump_until(&mut engine, is_waiting);

        // The rename worker finishes and its completion lands.
        crate::git::rename_branch(&worktree, "old", "new").unwrap();
        engine.process_worker_event(WorkerEvent::BranchRenameCompleted {
            session_id: "s-old".into(),
            new_branch: dispatch.new_branch.clone(),
            previous_title: dispatch.previous_title.clone(),
            result: Ok(()),
            status: crate::engine::ResolvedFinal::new("k", crate::engine::Final::clear()),
        });
        // The recorded request follows the rename too, so a crash before the
        // removal runs still deletes the branch by its new name next start.
        let pending = engine
            .session_store
            .load_pending_worktree_removals()
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].managed.branch_name, "new");

        let done = pump_until(&mut engine, is_completion);
        assert!(matches!(
            done,
            EventReaction::WorktreeRemoveSucceeded { .. }
        ));
        assert!(!worktree.exists());
        assert!(!branch_exists(&repo, "new"), "the renamed branch goes too");
        assert!(!branch_exists(&repo, "old"));
    }

    /// Item 8: a delete during "Recreate the working copy" waits for the
    /// recreate, then removes the copy it put back, instead of removing the
    /// missing folder and leaving the recreated one behind.
    #[test]
    fn a_delete_waits_for_a_recreate_and_removes_the_recreated_copy() {
        let (mut engine, tmp) = test_engine();
        let (repo, worktrees) = world(&mut engine, tmp.path(), &["agent"]);
        let worktree = worktrees[0].clone();
        // The copy is gone, and a recreate is running for it.
        git(
            &repo,
            &["worktree", "remove", "--force", worktree.to_str().unwrap()],
        );
        let recreate = InFlightKey::RecreateWorkingCopy("s-agent".into());
        engine.mark_in_flight(recreate.clone());
        engine
            .hold_path_for_in_flight(&recreate, &worktree, WorktreeOpKind::RecreateWorkingCopy)
            .unwrap();

        assert!(matches!(
            engine.begin_delete_session("s-agent", true, None),
            BeginDeleteSessionOutcome::AsyncStarted { .. }
        ));
        pump_until(&mut engine, is_waiting);

        // The recreate's checkout lands, then its completion.
        git(
            &repo,
            &["worktree", "add", worktree.to_str().unwrap(), "agent"],
        );
        engine.process_worker_event(WorkerEvent::WorkingCopyRecreated {
            session_id: "s-agent".into(),
            outcome: Ok(crate::working_copy::RecreatedBranch::CheckedOut),
        });

        pump_until(&mut engine, is_completion);
        assert!(!worktree.exists(), "the recreated copy is removed too");
    }

    /// Item 9: the deferred removal's last-moment occupancy check counts an
    /// agent being created on the worktree, not only agents already listed.
    #[test]
    fn a_deferred_removal_keeps_a_worktree_an_agent_is_being_created_on() {
        let (mut engine, tmp) = test_engine();
        let (_repo, worktrees) = world(&mut engine, tmp.path(), &["agent"]);
        let worktree = worktrees[0].clone();
        let managed = engine.sessions[0].workspace.as_managed().unwrap().clone();
        engine
            .worktree_ops()
            .hold_as(
                HoldOwner::CreateOp("op-1".into()),
                &worktree,
                WorktreeOpKind::CreateAgent,
            )
            .unwrap();
        engine.dispatch_deferred_worktree_removal(crate::engine::DeferredWorktreeRemoval {
            session_id: "s-agent".into(),
            project_path: engine.projects[0].path.clone(),
            managed,
            delete_branch: None,
            busy_message: "Removing\u{2026}".to_string().into(),
            processes: crate::engine::RemovalProcesses::none(),
        });
        let done = pump_until(&mut engine, is_completion);
        let EventReaction::WorktreeRemoveFailed { message, .. } = done else {
            panic!("a kept worktree answers the delete's spinner");
        };
        assert!(message.contains("being created"), "{message}");
        assert!(worktree.exists());
    }

    /// Item 2: a project delete removes its records at once, under ONE keyed
    /// busy, and each worktree through the deferred removal: a worktree with a
    /// pull in flight waits for it rather than being removed under it on the
    /// engine thread. One final, on the busy's key, ends it.
    #[test]
    fn a_project_delete_waits_for_each_worktree_and_reports_once() {
        let (mut engine, tmp) = test_engine();
        let (repo, worktrees) = world(&mut engine, tmp.path(), &["one", "two"]);
        let pull = InFlightKey::Pull(worktrees[0].to_string_lossy().into_owned());
        engine.mark_in_flight(pull.clone());
        engine
            .hold_path_for_in_flight(&pull, &worktrees[0], WorktreeOpKind::Pull)
            .unwrap();

        let reaction = engine
            .apply(Command::DeleteProject {
                project_id: "p1".into(),
                project_name: "demo".into(),
            })
            .unwrap();
        let started = statuses(&reaction);
        assert_eq!(started.len(), 1, "one status, the cascade's busy");
        assert_eq!(started[0].tone, StatusTone::Busy);
        let key = started[0].key.clone().expect("the busy is keyed");
        assert!(engine.projects.is_empty() && engine.sessions.is_empty());
        assert!(
            worktrees[0].exists(),
            "the held worktree waits for its pull"
        );

        // The free one goes on its own without a final of its own, and the
        // held one says, on the cascade's own spinner, what it waits for. The
        // two workers run side by side, so either can report first.
        let mut seen_waiting = false;
        let mut seen_completion = false;
        while !(seen_waiting && seen_completion) {
            let reaction = pump_until(&mut engine, |event| {
                let hit = is_waiting(event) || is_completion(event);
                if is_waiting(event) {
                    seen_waiting = true;
                }
                if is_completion(event) {
                    seen_completion = true;
                }
                hit
            });
            for status in statuses(&reaction) {
                assert_eq!(status.key.as_deref(), Some(key.as_str()));
                assert_eq!(status.tone, StatusTone::Busy, "{}", status.message);
                assert!(
                    status.message.contains("waiting for a pull"),
                    "{}",
                    status.message
                );
            }
        }
        assert!(!worktrees[1].exists());
        assert!(worktrees[0].exists());

        engine.clear_in_flight(&pull);
        let last = pump_until(&mut engine, is_completion);
        let finals = statuses(&last);
        assert_eq!(finals.len(), 1);
        assert_eq!(finals[0].key.as_deref(), Some(key.as_str()));
        assert_eq!(finals[0].tone, StatusTone::Info, "{}", finals[0].message);
        assert!(!worktrees[0].exists());
        assert!(!branch_exists(&repo, "one") && !branch_exists(&repo, "two"));
    }

    /// Item 2: a worktree that cannot be removed is named in the cascade's one
    /// final, with git's reason, and the other worktrees still go.
    #[test]
    fn a_project_delete_names_each_worktree_it_could_not_remove() {
        let (mut engine, tmp) = test_engine();
        let (repo, worktrees) = world(&mut engine, tmp.path(), &["one", "two"]);
        // A locked worktree refuses a single `--force` removal.
        git(&repo, &["worktree", "lock", worktrees[1].to_str().unwrap()]);

        engine
            .apply(Command::DeleteProject {
                project_id: "p1".into(),
                project_name: "demo".into(),
            })
            .unwrap();
        pump_until(&mut engine, is_completion);
        let last = pump_until(&mut engine, is_completion);
        let finals = statuses(&last);
        assert_eq!(finals.len(), 1);
        assert_eq!(finals[0].tone, StatusTone::Warning);
        assert!(finals[0].sticky, "a folder was left behind for the user");
        assert!(finals[0].message.contains("two"), "{}", finals[0].message);
        assert!(
            finals[0].message.contains("locked"),
            "{}",
            finals[0].message
        );
        assert!(!worktrees[0].exists());
        assert!(worktrees[1].exists());
    }

    /// Item 2: a live agent's worktree is not removed while its process is in
    /// its grace period; the cascade goes through the same deferred path a
    /// single delete takes.
    #[test]
    fn a_project_delete_lets_a_running_agent_exit_before_removing_its_worktree() {
        let (mut engine, tmp) = test_engine();
        let (_repo, worktrees) = world(&mut engine, tmp.path(), &["agent"]);
        let tab = engine.sessions[0].slot_tab_id().to_owned();
        let client = crate::pty::PtyClient::spawn_with_env(
            "sh",
            &["-c".to_string(), "trap '' TERM; sleep 30".to_string()],
            &worktrees[0],
            24,
            80,
            100,
            &[],
        )
        .expect("spawn");
        engine.providers.insert(tab, client);

        engine
            .apply(Command::DeleteProject {
                project_id: "p1".into(),
                project_name: "demo".into(),
            })
            .unwrap();
        assert_eq!(
            engine.terminating_ptys.len(),
            1,
            "the agent was asked to stop"
        );
        assert!(worktrees[0].exists(), "and its worktree waits for it");
    }

    /// Item 7: a project delete while an agent is being created in it waits for
    /// that create, then takes the new agent with the rest.
    #[test]
    fn a_project_delete_waits_for_an_agent_being_created_in_it() {
        let (mut engine, tmp) = test_engine();
        let (_repo, worktrees) = world(&mut engine, tmp.path(), &["old", "new"]);
        // `s-new` is the agent being created: not listed yet.
        let created = engine.sessions.pop().unwrap();
        engine.note_create_started("op-1", Some("p1"));
        engine.mark_in_flight(InFlightKey::CreateAgent);

        let reaction = engine
            .apply(Command::DeleteProject {
                project_id: "p1".into(),
                project_name: "demo".into(),
            })
            .unwrap();
        let busy = statuses(&reaction);
        assert!(
            busy[0].message.contains("being created"),
            "{}",
            busy[0].message
        );
        assert_eq!(
            engine.projects.len(),
            1,
            "nothing goes while the create runs"
        );
        assert!(worktrees[0].exists());

        // The create lands, the way its launch-ready does.
        engine.clear_in_flight(&InFlightKey::CreateAgent);
        engine.sessions.insert(0, created);
        engine.note_create_finished("op-1");
        pump_until(&mut engine, |event| {
            matches!(event, WorkerEvent::ProjectDeletionContinue { .. })
        });
        assert!(engine.projects.is_empty() && engine.sessions.is_empty());
        pump_until(&mut engine, is_completion);
        pump_until(&mut engine, is_completion);
        assert!(!worktrees[0].exists() && !worktrees[1].exists());
    }

    /// Item 7: no new agent is created in a project being deleted.
    #[test]
    fn no_agent_is_created_in_a_project_being_deleted() {
        let (mut engine, tmp) = test_engine();
        world(&mut engine, tmp.path(), &[]);
        engine.note_create_started("op-1", Some("p1"));
        engine.mark_in_flight(InFlightKey::CreateAgent);
        engine
            .apply(Command::DeleteProject {
                project_id: "p1".into(),
                project_name: "demo".into(),
            })
            .unwrap();
        engine.clear_in_flight(&InFlightKey::CreateAgent);
        let project = engine.projects[0].clone();
        let reaction = engine
            .apply(Command::DispatchCreateAgentRequest {
                request: Box::new(crate::worker::CreateAgentRequest::NewProject {
                    project,
                    custom_name: Some("x".into()),
                    use_existing_branch: false,
                    pull_before_create: false,
                    copy_uncommitted_changes: false,
                }),
                busy_message: "Creating\u{2026}".to_string().into(),
                term_size: (80, 24),
            })
            .unwrap();
        let refused = statuses(&reaction);
        assert_eq!(refused[0].tone, StatusTone::Error);
        assert!(refused[0].message.contains("is being deleted"));
    }

    /// Item 7's defence: a create that outlasted the wait lands for a project
    /// that is gone. Nothing is committed, the process is stopped, and the
    /// worktree this launch made goes with the branch dux minted, once the
    /// process has exited.
    #[test]
    fn a_launch_for_a_deleted_project_is_stopped_and_its_worktree_taken_back() {
        let (mut engine, tmp) = test_engine();
        let (repo, worktrees) = world(&mut engine, tmp.path(), &["late"]);
        let session = engine.sessions.pop().unwrap();
        engine.projects.clear();
        let client =
            crate::pty::PtyClient::spawn_with_env("cat", &[], &worktrees[0], 24, 80, 100, &[])
                .unwrap();
        let tab = session.slot_tab_id().to_owned();
        let (outcome, final_status) =
            engine.process_agent_launch_ready(crate::worker::AgentLaunchReadyData {
                request: crate::worker::AgentLaunchRequest {
                    tab_id: tab,
                    provider: session.provider.clone(),
                    session,
                    provider_config: Default::default(),
                    env: Vec::new(),
                    identity: Default::default(),
                    resume: false,
                    pty_size: (24, 80),
                    scrollback_lines: 100,
                    kind: crate::worker::AgentLaunchKind::Create {
                        status_message: Default::default(),
                        status_warns: false,
                        status_notes: None,
                        pull_request_pin: None,
                        repo_path: repo.to_string_lossy().into_owned(),
                        owns_worktree: true,
                        startup_result: None,
                        status_op_id: "op-late".into(),
                    },
                    wants_fullscreen: false,
                    status_quiet: crate::statusline::QuietSurfaces::LOUD,
                },
                client,
            });
        assert!(matches!(
            outcome.view,
            crate::engine::AgentLaunchReadyView::CreatePersistFailed { .. }
        ));
        assert!(final_status.is_none(), "no op was stashed for this create");
        assert!(engine.sessions.is_empty(), "nothing is committed");
        assert!(engine.providers.is_empty());
        assert_eq!(
            engine.terminating_ptys.len(),
            1,
            "the process was asked to stop"
        );
        assert!(worktrees[0].exists(), "not before the process has exited");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut removals = Vec::new();
        while removals.is_empty() && std::time::Instant::now() < deadline {
            removals = engine.reap_terminating_ptys().removals;
            std::thread::sleep(Duration::from_millis(50));
        }
        for removal in removals {
            engine.dispatch_deferred_worktree_removal(removal);
        }
        let done = pump_until(&mut engine, is_completion);
        assert!(matches!(
            done,
            EventReaction::WorktreeRemoveSucceeded {
                branches: RemovedBranches::Deleted(_),
                ..
            }
        ));
        assert!(!worktrees[0].exists());
        assert!(!branch_exists(&repo, "late"));
    }

    /// Item 9: a worktree whose agent was just deleted is listed as being
    /// removed and cannot be removed a second time from the manager.
    #[test]
    fn the_manager_shows_a_pending_removal_and_refuses_a_second_one() {
        let (mut engine, tmp) = test_engine();
        let (_repo, worktrees) = world(&mut engine, tmp.path(), &["agent"]);
        let worktree = worktrees[0].clone();
        // The agent's delete began: its record is gone, its removal pending.
        let _claim = engine.worktree_ops().announce_removal(&worktree);
        engine.sessions.clear();

        let project = engine.projects[0].clone();
        let listed = crate::worktree_manager::list_manageable_worktrees(
            &project,
            &engine.paths,
            &engine.sessions,
            engine.worktree_ops(),
        )
        .unwrap();
        assert!(listed[0].being_removed && !listed[0].is_removable());

        let admission = engine
            .admit_manager_removal("p1", &listed[0].path, true)
            .unwrap();
        assert!(matches!(
            admission,
            crate::worktree_manager::RemovalAdmission::Refused(
                crate::worktree_manager::RemovalOutcome::BeingRemoved
            )
        ));
        assert!(worktree.exists());
    }

    /// Item 9: "attached" is judged at the moment of removal, from live state:
    /// an agent being created on the worktree counts, though no snapshot of
    /// the sessions lists it yet.
    #[test]
    fn the_manager_refuses_a_worktree_an_agent_is_being_created_on() {
        let (mut engine, tmp) = test_engine();
        let (_repo, worktrees) = world(&mut engine, tmp.path(), &["free"]);
        engine.sessions.clear();
        engine
            .worktree_ops()
            .hold_as(
                HoldOwner::CreateOp("op-1".into()),
                &worktrees[0],
                WorktreeOpKind::CreateAgent,
            )
            .unwrap();
        let admission = engine
            .admit_manager_removal("p1", &worktrees[0], true)
            .unwrap();
        assert!(matches!(
            admission,
            crate::worktree_manager::RemovalAdmission::Refused(
                crate::worktree_manager::RemovalOutcome::Attached
            )
        ));
    }

    /// Item 9: an admitted manager removal waits for the operations running in
    /// the worktree, and refuses new ones from the moment it was admitted.
    #[test]
    fn a_manager_removal_waits_for_work_in_the_worktree() {
        let (mut engine, tmp) = test_engine();
        let (_repo, worktrees) = world(&mut engine, tmp.path(), &["free"]);
        engine.sessions.clear();
        let guard = engine
            .worktree_ops()
            .hold(&worktrees[0], WorktreeOpKind::Push)
            .unwrap();
        let crate::worktree_manager::RemovalAdmission::Admitted(ticket) = engine
            .admit_manager_removal("p1", &worktrees[0], false)
            .unwrap()
        else {
            panic!("a free worktree is admitted");
        };
        assert_eq!(ticket.waiting_for().as_deref(), Some("a push"));
        assert!(
            engine
                .worktree_ops()
                .hold(&worktrees[0], WorktreeOpKind::EditorWrite)
                .is_err(),
            "nothing new starts once the removal is admitted"
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(ticket.run()).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(worktrees[0].exists());
        drop(guard);
        let outcome = rx.recv_timeout(Duration::from_secs(20)).unwrap().unwrap();
        assert!(matches!(
            outcome,
            crate::worktree_manager::RemovalOutcome::Removed { .. }
        ));
        assert!(!worktrees[0].exists());
    }
}
