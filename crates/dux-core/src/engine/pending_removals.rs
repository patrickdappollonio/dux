//! Worktree removals that must survive dux quitting in the middle of them.
//!
//! A delete with "delete the worktree" removes the agent's record at once and
//! the worktree only after the agent's processes are gone, which takes up to
//! the close grace. A quit, a crash or a forced exit inside that window used
//! to lose the removal: the worktree and the branch stayed, and no agent was
//! left to delete them from. The request is therefore written to SQLite when
//! the delete is accepted and cleared when the removal has run. A clean quit
//! finishes it inside the shutdown wait; anything else is finished by the next
//! start, in a background worker, with a status saying so.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::engine::events::{RemovedBranches, StatusUpdate, perform_deferred_removal};
use crate::engine::{DeferredWorktreeRemoval, Engine, RemovalProcesses};
use crate::status_text::StatusText;
use crate::storage::PendingWorktreeRemoval;
use crate::worker::WorkerEvent;

/// How long a quit waits for git once the agent's processes are gone. git's
/// own work on a worktree is seconds at most; this bounds a git that hangs on
/// a lock rather than an ordinary removal, which is then finished by the next
/// start instead.
const QUIT_GIT_WAIT: Duration = Duration::from_secs(30);

impl Engine {
    /// What occupies `folder` (the folder itself, or anything inside it) for
    /// a removal, or `None` when it is free. THE occupancy question: the agent
    /// delete (at dispatch, under its claim), a removal finished at the next
    /// start, the project cascade (which goes through the agent delete) and the
    /// worktree manager all ask this, so they cannot disagree about what is in
    /// the way. In order:
    ///
    /// - an agent record, live or dormant, managed or standalone, other than
    ///   `removing`, whose folder is in or under `folder`. A standalone one is
    ///   reported first: dux never removes its folder or stops it to make way;
    /// - an agent being created in it (a create holds its path until its launch
    ///   lands in `sessions`);
    /// - a live PTY dux runs in it (a terminal, a tab);
    /// - a PTY still stopping in it, when `stopping` says those occupy. An agent
    ///   delete ENDS the stopping processes of agents already deleted (a
    ///   sibling that shared the worktree); the manager, which ends nothing,
    ///   treats them as in the way.
    pub(crate) fn folder_occupant(
        &self,
        folder: &std::path::Path,
        removing: Option<&str>,
        stopping: StoppingProcesses,
    ) -> Option<Occupant> {
        let inside = |dir: &std::path::Path| crate::worktree_ops::folder_contains(folder, dir);
        let agents: Vec<&crate::model::AgentSession> = self
            .sessions
            .iter()
            .filter(|s| Some(s.id.as_str()) != removing)
            .filter(|s| inside(std::path::Path::new(s.directory())))
            .collect();
        if let Some(agent) = agents
            .iter()
            .find(|s| s.workspace.as_managed().is_none())
            .or_else(|| agents.first())
        {
            return Some(Occupant::Agent {
                id: agent.id.clone(),
                label: agent.display_label(),
                directory: agent.directory().to_string(),
                standalone: agent.workspace.as_managed().is_none(),
                exact: crate::worktree_ops::path_key(std::path::Path::new(agent.directory()))
                    == crate::worktree_ops::path_key(folder),
            });
        }
        if self
            .removal_coordination
            .ops
            .holders(folder)
            .contains(&crate::worktree_ops::WorktreeOpKind::CreateAgent)
        {
            return Some(Occupant::BeingCreated);
        }
        self.pty_occupants_in(folder)
            .into_iter()
            .find(|(_, _, terminating)| {
                !terminating || matches!(stopping, StoppingProcesses::Occupy)
            })
            .map(|(_, what, _)| Occupant::Process(what))
    }

    /// The sentence for an agent delete's removal that must not run because the
    /// folder is occupied (see [`Self::folder_occupant`]), or `None` when it is
    /// free.
    pub(crate) fn worktree_occupied_message(
        &self,
        session_id: &str,
        worktree_path: &str,
    ) -> Option<StatusText> {
        let folder = std::path::Path::new(worktree_path);
        let occupant = self.folder_occupant(folder, Some(session_id), StoppingProcesses::Ended)?;
        Some(occupant.kept_message(&crate::home_path::shorten_home(folder)))
    }

    /// Write a removal down until it has run. A failure to write is logged and
    /// the removal goes ahead: losing it only on a crash is no worse than
    /// before.
    pub(crate) fn record_pending_removal(&self, label: &str, removal: &DeferredWorktreeRemoval) {
        // Everything the live dispatch would end, so a removal finished at the
        // next start ends exactly the same set: the agent's own sessions, and
        // every session dux registered in the folder (a closed terminal's, an
        // agent deleted earlier with its worktree kept), with what dux has
        // already recorded as running in them. The snapshot thread adds what
        // it sees once it has looked, and a survivor recorded later for this
        // folder is added as it lands.
        let mut sessions = removal.processes.sessions.clone();
        sessions
            .extend(self.process_sessions_in(std::path::Path::new(&removal.managed.worktree_path)));
        sessions.sort_by_key(|session| session.sid);
        sessions.dedup();
        let mut known = removal
            .processes
            .snapshot
            .get()
            .cloned()
            .unwrap_or_default();
        known.extend(self.process_registry.survivors_of(&sessions));
        known.sort_by_key(|identity| (identity.pid, identity.start_time));
        known.dedup();
        let row = PendingWorktreeRemoval {
            session_id: removal.session_id.clone(),
            label: label.to_string(),
            project_path: removal.project_path.clone(),
            managed: removal.managed.clone(),
            delete_branch: removal.delete_branch,
            process_sessions: sessions,
            process_snapshot: known,
        };
        if let Err(err) = self.session_store.insert_pending_worktree_removal(&row) {
            crate::logger::error(&format!(
                "could not record the worktree removal of agent {} to finish after a quit: {err:#}",
                removal.session_id
            ));
        }
        self.process_registry
            .set_pending_removal_store(&self.paths.sessions_db_path);
    }

    /// Clear a pending removal's row on the engine's own connection.
    pub(crate) fn forget_pending_removal(&self, session_id: &str) {
        if let Err(err) = self
            .session_store
            .delete_pending_worktree_removal(session_id)
        {
            crate::logger::warn(&format!(
                "could not clear the worktree removal of agent {session_id}: {err:#}"
            ));
        }
    }

    /// Finish, before dux exits, every worktree removal a delete has started:
    /// wait out the agents' processes (the reaper force-kills each at its own
    /// deadline), hand the removals to their workers, and wait for those, all
    /// bounded. Called from the shutdown wait, so a clean quit leaves nothing
    /// half done. A second interrupt (`abort`) stops waiting; whatever is left
    /// stays recorded and the next start finishes it.
    pub fn finish_pending_worktree_removals(&mut self, abort: Option<&AtomicBool>) {
        let aborted = || abort.is_some_and(|flag| flag.load(Ordering::SeqCst));
        if self.pending_group_removals.is_empty()
            && self.removal_workers.iter().all(|w| w.is_finished())
        {
            return;
        }
        let waiting = self.pending_group_removals.len()
            + self
                .removal_workers
                .iter()
                .filter(|w| !w.is_finished())
                .count();
        crate::logger::info(&format!(
            "Finishing {} of deleted agents before quitting.",
            crate::text::count_of_with(waiting, "worktree removal", "worktree removals")
        ));
        let barrier_deadline = self
            .terminating_ptys
            .iter()
            .map(|entry| entry.deadline)
            .max()
            .unwrap_or_else(Instant::now)
            + crate::process_sessions::KILL_SETTLE;
        loop {
            for removal in self.reap_terminating_ptys().removals {
                let _ = self.dispatch_deferred_worktree_removal(removal);
            }
            if self.pending_group_removals.is_empty()
                || aborted()
                || Instant::now() >= barrier_deadline
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let worker_deadline = Instant::now()
            + self.individual_close_grace()
            + crate::process_sessions::KILL_SETTLE
            + QUIT_GIT_WAIT;
        while self.removal_workers.iter().any(|w| !w.is_finished())
            && !aborted()
            && Instant::now() < worker_deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        let unfinished = self.pending_group_removals.len()
            + self
                .removal_workers
                .iter()
                .filter(|w| !w.is_finished())
                .count();
        if unfinished == 0 {
            crate::logger::info("Finished every worktree removal before quitting.");
        } else {
            crate::logger::warn(&format!(
                "Quitting with {} unfinished; dux finishes them the next time it starts.",
                crate::text::count_of_with(unfinished, "worktree removal", "worktree removals")
            ));
        }
    }

    /// Finish the worktree removals an earlier run of dux accepted and never
    /// completed (it quit, crashed or was killed in the middle), each in a
    /// background worker, with a status on both surfaces. A removal whose
    /// directory another agent now works in is refused out loud and forgotten.
    pub fn resume_pending_worktree_removals(&mut self) {
        let rows = match self.session_store.load_pending_worktree_removals() {
            Ok(rows) => rows,
            Err(err) => {
                crate::logger::error(&format!(
                    "could not read the worktree removals left from the last run: {err:#}"
                ));
                return;
            }
        };
        for row in rows {
            // The record outlived the delete (its own removal failed), so the
            // agent is still on screen and its worktree is not dux's to take.
            if self.sessions.iter().any(|s| s.id == row.session_id) {
                crate::logger::warn(&format!(
                    "not finishing the worktree removal of agent {}: the agent is still here",
                    row.session_id
                ));
                self.forget_pending_removal(&row.session_id);
                continue;
            }
            if let Some(message) =
                self.worktree_occupied_message(&row.session_id, &row.managed.worktree_path)
            {
                crate::logger::warn(&message);
                let _ = self
                    .worker_tx
                    .send(WorkerEvent::PollerStatus(StatusUpdate::warning(message)));
                self.forget_pending_removal(&row.session_id);
                continue;
            }
            self.spawn_resumed_removal(row);
        }
    }

    fn spawn_resumed_removal(&mut self, row: PendingWorktreeRemoval) {
        let path = crate::home_path::shorten_home(std::path::Path::new(&row.managed.worktree_path));
        let success_label = row.label.clone();
        let success_path = path.clone();
        let failure_label = row.label.clone();
        let failure_path = path.clone();
        let op = crate::engine::status_op(crate::status_text![
            "Finishing the removal of the worktree of deleted agent ",
            q(row.label.clone()),
            " at ",
            q(path),
            ", which dux had not finished when it last quit\u{2026}"
        ])
        .on_success(move |_: &RemovedBranches| {
            crate::engine::Final::info(crate::status_text![
                "Finished removing the worktree of deleted agent ",
                q(success_label.clone()),
                " at ",
                q(success_path.clone()),
                ", which dux had not finished when it last quit."
            ])
        })
        .on_failure(move |err: &String| {
            // STICKY: something is left on disk that the user has to deal
            // with, and the message says what.
            crate::engine::Final::error(crate::status_text![
                "Could not finish removing the worktree of deleted agent ",
                q(failure_label.clone()),
                " at ",
                q(failure_path.clone()),
                format!(", left over from the last time dux quit: {err}")
            ])
            .sticky()
        });
        // The busy goes out on the worker lane ahead of the worker, so it
        // reaches whichever surface drains first and always precedes its final.
        let _ = self
            .worker_tx
            .send(WorkerEvent::PollerStatus(op.pending_status()));
        let mut sessions = row.process_sessions.clone();
        sessions.extend(self.process_sessions_in(std::path::Path::new(&row.managed.worktree_path)));
        sessions.sort_by_key(|session| session.sid);
        sessions.dedup();
        // Sessions an earlier run recorded: a leaderless one is acted on only
        // through the processes the delete saw in it, never on its number,
        // which an unrelated program may hold by now. A session from another
        // boot is void altogether.
        let snapshot = std::sync::OnceLock::new();
        let _ = snapshot.set(row.process_snapshot.clone());
        let processes = RemovalProcesses {
            sessions,
            snapshot: std::sync::Arc::new(snapshot),
            grace: self.individual_close_grace(),
        };
        let registry = self.process_registry.clone();
        let db_path = self.paths.sessions_db_path.clone();
        // The same claim a delete takes, so nothing new starts in the folder
        // while it is being finished, and a second removal of it joins this one.
        let claim = self
            .removal_coordination
            .ops
            .announce_removal(&row.managed.worktree_path);
        let wait = self.removal_wait();
        let _ = self.spawn_status_op(op, move || {
            let result = perform_deferred_removal(
                &row.session_id,
                &row.project_path,
                &row.managed,
                row.delete_branch,
                &processes,
                super::events::RemovalCoordinationInputs {
                    claim,
                    wait,
                    waiting_tx: None,
                    registry,
                },
            );
            super::events::forget_pending_removal_in(&db_path, &row.session_id);
            result
        });
    }
}

impl Engine {
    /// Every PTY dux still has a process for in `folder` or inside it, whoever
    /// owns it: a live agent tab, a terminal, or one that is terminating (an
    /// agent deleted a moment ago, worktree kept, whose CLI is still stopping).
    /// Read from the engine's own maps, so it needs no look at the process
    /// table and is cheap on the engine thread. Each comes with what to call it
    /// and whether it is stopping.
    pub fn pty_occupants_in(
        &self,
        folder: &std::path::Path,
    ) -> Vec<(
        Option<crate::process_sessions::ProcessSession>,
        &'static str,
        bool,
    )> {
        self.pty_occupants()
            .into_iter()
            .filter(|(dir, ..)| crate::worktree_ops::folder_contains(folder, dir))
            .map(|(_, session, what, terminating)| (session, what, terminating))
            .collect()
    }

    /// Every PTY dux has a process for, with the folder it started in.
    fn pty_occupants(
        &self,
    ) -> Vec<(
        std::path::PathBuf,
        Option<crate::process_sessions::ProcessSession>,
        &'static str,
        bool,
    )> {
        let mut found = Vec::new();
        for client in self.providers.values() {
            found.push((
                client.spawn_dir().to_path_buf(),
                client.process_session(),
                "an agent running in it",
                false,
            ));
        }
        for terminal in self.companion_terminals.values() {
            found.push((
                terminal.client.spawn_dir().to_path_buf(),
                terminal.client.process_session(),
                "a terminal open in it",
                false,
            ));
        }
        for entry in &self.terminating_ptys {
            found.push((
                entry.client.spawn_dir().to_path_buf(),
                entry.client.process_session(),
                "a process dux started there that is still stopping",
                true,
            ));
        }
        found
    }

    /// Every process session a removal of `folder` must end first: each one
    /// dux registered as started there (any agent, a deleted one included,
    /// terminals and startup commands too) and each PTY still running there.
    pub fn process_sessions_in(
        &self,
        folder: &std::path::Path,
    ) -> Vec<crate::process_sessions::ProcessSession> {
        let mut sessions = self.process_registry.sessions_in(folder);
        sessions.extend(
            self.pty_occupants_in(folder)
                .into_iter()
                .filter_map(|(session, ..)| session),
        );
        sessions.sort_by_key(|session| session.sid);
        sessions.dedup();
        sessions
    }

    /// Every folder something occupies, with why, from the same sources as
    /// [`Self::folder_occupant`] (as the manager sees them: stopping processes
    /// occupy). The worktree manager's listing marks a row in use when one of
    /// these is the row's folder or inside it; a row whose own agent holds it
    /// is listed as held instead.
    pub fn busy_folders(&self) -> Vec<(std::path::PathBuf, String)> {
        let mut found: Vec<(std::path::PathBuf, String)> = self
            .sessions
            .iter()
            .map(|agent| {
                (
                    crate::worktree_ops::path_key(std::path::Path::new(agent.directory())),
                    Occupant::Agent {
                        id: agent.id.clone(),
                        label: agent.display_label(),
                        directory: agent.directory().to_string(),
                        standalone: agent.workspace.as_managed().is_none(),
                        exact: false,
                    }
                    .reason(),
                )
            })
            .collect();
        found.extend(
            self.pty_occupants()
                .into_iter()
                .map(|(dir, _, what, _)| (crate::worktree_ops::path_key(&dir), what.to_string())),
        );
        found
    }
}

/// Whether processes still stopping in a folder are in the way of removing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoppingProcesses {
    /// The removal ends them first (an agent delete).
    Ended,
    /// They occupy the folder (the worktree manager, which ends nothing).
    Occupy,
}

/// What [`Engine::folder_occupant`] found in the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Occupant {
    Agent {
        id: String,
        label: String,
        directory: String,
        standalone: bool,
        /// The agent's folder IS the folder asked about (it holds it), rather
        /// than one inside it.
        exact: bool,
    },
    BeingCreated,
    Process(&'static str),
}

impl Occupant {
    /// What is in the way, as a phrase for the manager's row and refusal.
    pub(crate) fn reason(&self) -> String {
        let folder =
            |directory: &str| crate::home_path::shorten_home(std::path::Path::new(directory));
        match self {
            Occupant::Agent {
                label,
                directory,
                standalone: true,
                ..
            } => format!(
                "standalone agent \"{label}\" runs in {}; delete that agent, or move its folder out, first",
                folder(directory)
            ),
            Occupant::Agent {
                label,
                directory,
                standalone: false,
                ..
            } => format!(
                "agent \"{label}\" has its worktree at {}; delete that agent first",
                folder(directory)
            ),
            Occupant::BeingCreated => "an agent is being created in it".to_string(),
            Occupant::Process(what) => (*what).to_string(),
        }
    }

    /// The final for an agent delete's removal kept because of this.
    fn kept_message(&self, worktree: &str) -> StatusText {
        match self {
            Occupant::Agent {
                label,
                directory,
                standalone: true,
                ..
            } => crate::status_text![
                "Kept the worktree at ",
                q(worktree.to_string()),
                ": standalone agent ",
                q(label.clone()),
                " runs in ",
                n(crate::home_path::shorten_home(std::path::Path::new(
                    directory
                ))),
                ", inside it, and dux never removes a standalone agent's folder or stops it to \
                 make way. Delete that agent, or move its folder out of the worktree, then \
                 remove the worktree from the worktree manager."
            ],
            Occupant::Agent {
                label,
                directory,
                exact: false,
                ..
            } => crate::status_text![
                "Kept the worktree at ",
                q(worktree.to_string()),
                ": agent ",
                q(label.clone()),
                " has its own worktree at ",
                n(crate::home_path::shorten_home(std::path::Path::new(
                    directory
                ))),
                ", inside it, and removing this one would delete that one too. Delete that \
                 agent first, then remove this worktree from the worktree manager."
            ],
            Occupant::Agent { label, .. } => crate::status_text![
                "Kept the worktree at ",
                q(worktree.to_string()),
                ": agent ",
                q(label.clone()),
                " started working in it while this agent was shutting down. Remove it from \
                 the worktree manager if you still want it gone."
            ],
            Occupant::BeingCreated => crate::status_text![
                "Kept the worktree at ",
                q(worktree.to_string()),
                ": an agent is being created in it while this agent was shutting down. Remove \
                 it from the worktree manager if you still want it gone."
            ],
            Occupant::Process(what) => crate::status_text![
                "Kept the worktree at ",
                q(worktree.to_string()),
                format!(
                    ": {what}. Remove it from the worktree manager once that has stopped, if \
                     you still want it gone."
                )
            ],
        }
    }
}
