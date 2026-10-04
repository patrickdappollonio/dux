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
    /// The sentence for a removal that must not run because another agent now
    /// works in the same directory, or `None` when nobody does.
    pub(crate) fn worktree_occupied_message(
        &self,
        session_id: &str,
        worktree_path: &str,
    ) -> Option<StatusText> {
        let occupant = self.sessions.iter().find(|s| {
            s.id != session_id
                && crate::project_browser::same_directory(s.directory(), worktree_path)
        })?;
        Some(crate::status_text![
            "Kept the worktree at ",
            q(crate::home_path::shorten_home(std::path::Path::new(
                worktree_path
            ))),
            ": agent ",
            q(occupant.display_label()),
            " started working in it while this \
             agent was shutting down. Remove it from the worktree manager if you still \
             want it gone."
        ])
    }

    /// Write a removal down until it has run. A failure to write is logged and
    /// the removal goes ahead: losing it only on a crash is no worse than
    /// before.
    pub(crate) fn record_pending_removal(&self, label: &str, removal: &DeferredWorktreeRemoval) {
        let row = PendingWorktreeRemoval {
            session_id: removal.session_id.clone(),
            label: label.to_string(),
            project_path: removal.project_path.clone(),
            managed: removal.managed.clone(),
            delete_branch: removal.delete_branch,
            process_sessions: removal.processes.sessions.clone(),
        };
        if let Err(err) = self.session_store.insert_pending_worktree_removal(&row) {
            crate::logger::error(&format!(
                "could not record the worktree removal of agent {} to finish after a quit: {err:#}",
                removal.session_id
            ));
        }
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
        let processes = RemovalProcesses {
            sessions: row.process_sessions.clone(),
            grace: self.individual_close_grace(),
            ..RemovalProcesses::none()
        };
        let db_path = self.paths.sessions_db_path.clone();
        let _ = self.spawn_status_op(op, move || {
            let result = perform_deferred_removal(
                &row.session_id,
                &row.project_path,
                &row.managed,
                row.delete_branch,
                &processes,
            );
            super::events::forget_pending_removal_in(&db_path, &row.session_id);
            result
        });
    }
}
