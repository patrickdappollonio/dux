//! Deleting the folder dux created for a standalone agent once its record is
//! gone and its processes have stopped, after the checks a worktree removal makes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::agent_folders::FolderKept;
use crate::engine::events::RemovalPlace;
use crate::engine::{Engine, Final};
use crate::model::DirectoryIdentity;
use crate::storage::PendingFolderRemoval;
use crate::worker::WorkerEvent;

/// One folder dux created, to delete once its agent's processes are gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedFolderRemoval {
    /// The deleted agent, which is not an occupant of its own folder.
    pub session_id: String,
    /// The folder.
    pub path: PathBuf,
    /// The identity it had when dux created it.
    pub identity: DirectoryIdentity,
    /// The deleted agent's name, for the status.
    pub label: String,
}

impl CreatedFolderRemoval {
    fn row(&self) -> PendingFolderRemoval {
        PendingFolderRemoval {
            session_id: self.session_id.clone(),
            label: self.label.clone(),
            path: self.path.to_string_lossy().into_owned(),
            identity: self.identity,
        }
    }
}

/// A folder removal held from the start of its agent's delete until it runs.
#[derive(Debug)]
pub struct GroupFolderRemoval {
    /// The tabs and terminals of the deleted agent still terminating.
    pub pending_ids: HashSet<String>,
    /// The removal to run once they are all gone.
    pub removal: CreatedFolderRemoval,
    /// Whether the agent's record is gone and the removal written down in
    /// its place. Nothing is deleted before.
    pub record_gone: bool,
    /// The claim on the folder, taken when the delete began.
    claim: crate::worktree_ops::RemovalClaim,
}

impl Engine {
    /// Every PTY id of the agent: its live tabs, its terminals, and its PTYs
    /// already terminating.
    pub(crate) fn session_pty_ids(&self, session_id: &str) -> HashSet<String> {
        let mut ids: HashSet<String> = self
            .tab_ids_for_session(session_id)
            .into_iter()
            .filter(|id| self.providers.contains_key(id.as_ref_id()))
            .map(|id| id.as_str().to_string())
            .collect();
        ids.extend(
            self.companion_terminals
                .iter()
                .filter(|(_, terminal)| terminal.owner.closed_by_session_delete(session_id))
                .map(|(id, _)| id.clone()),
        );
        ids.extend(
            self.terminating_ptys
                .iter()
                .filter(|entry| {
                    entry
                        .owner
                        .as_ref()
                        .is_some_and(|owner| owner.session_id == session_id)
                })
                .map(|entry| entry.id.clone()),
        );
        ids
    }

    /// Hold the removal of the folder of an agent whose delete has begun:
    /// the folder is claimed now, so nothing new can start in it, and nothing
    /// is deleted until [`Self::settle_held_folder_removal`] says the record
    /// is gone.
    pub(crate) fn hold_created_folder_removal(
        &mut self,
        removal: CreatedFolderRemoval,
        pty_ids: HashSet<String>,
    ) {
        self.pending_folder_removals
            .retain(|group| group.removal.session_id != removal.session_id);
        let claim = self
            .removal_coordination
            .ops
            .announce_removal(&removal.path);
        self.pending_folder_removals.push(GroupFolderRemoval {
            pending_ids: pty_ids,
            removal,
            record_gone: false,
            claim,
        });
    }

    /// The row to write down with the deletion of `session_id`'s record, when
    /// a removal of its folder is held.
    pub(crate) fn held_folder_removal_row(&self, session_id: &str) -> Option<PendingFolderRemoval> {
        self.pending_folder_removals
            .iter()
            .find(|group| !group.record_gone && group.removal.session_id == session_id)
            .map(|group| group.removal.row())
    }

    /// Where the folder dux created for `session_id` is, and its mark, when
    /// the agent's delete keeps it.
    pub(crate) fn kept_created_folder(
        &self,
        session_id: &str,
    ) -> Option<(PathBuf, Option<crate::model::FolderMark>)> {
        if self.held_folder_removal_row(session_id).is_some() {
            return None;
        }
        let session = self.sessions.iter().find(|s| s.id == session_id)?;
        match session.workspace.folder_removal_right() {
            crate::model::FolderRemovalRight::CreatedByDux { path, identity } => {
                Some((PathBuf::from(path), identity.mark))
            }
            crate::model::FolderRemovalRight::ChosenByUser { .. }
            | crate::model::FolderRemovalRight::NotAFolder => None,
        }
    }

    /// Let go of a folder dux created and keeps, on a worker.
    pub(crate) fn let_go_of_created_folder(
        &mut self,
        path: PathBuf,
        mark: Option<crate::model::FolderMark>,
    ) {
        let spawned = super::spawn_worker::spawn_engine_thread("dux-folder-let-go", move || {
            crate::agent_folders::let_go(&path, mark);
        });
        self.removal_workers.retain(|worker| !worker.is_finished());
        match spawned {
            Ok(handle) => self.removal_workers.push(handle),
            Err(err) => crate::logger::warn(&format!(
                "could not start the work that removes a kept folder's marker file: {err}"
            )),
        }
    }

    /// The deletion of `session_id`'s record has ended: when the record is
    /// gone the held removal waits only for the agent's processes; when it is
    /// not, the removal is let go and the folder stays with its agent.
    pub(crate) fn settle_held_folder_removal(&mut self, session_id: &str, record_gone: bool) {
        if !record_gone {
            self.pending_folder_removals
                .retain(|group| group.record_gone || group.removal.session_id != session_id);
            return;
        }
        let terminating: HashSet<&str> = self
            .terminating_ptys
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        for group in &mut self.pending_folder_removals {
            if group.removal.session_id == session_id {
                group.record_gone = true;
                group
                    .pending_ids
                    .retain(|id| terminating.contains(id.as_str()));
            }
        }
        self.release_folder_removals(&[]);
    }

    /// How many folder removals are waiting for their agent's processes.
    pub(crate) fn folder_removals_waiting(&self) -> usize {
        self.pending_folder_removals
            .iter()
            .filter(|group| group.record_gone)
            .count()
    }

    /// Release the folder removals whose last PTY is among `reaped_ids`.
    pub(crate) fn release_folder_removals(&mut self, reaped_ids: &[String]) {
        if self.pending_folder_removals.is_empty() {
            return;
        }
        for group in &mut self.pending_folder_removals {
            for id in reaped_ids {
                group.pending_ids.remove(id);
            }
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_folder_removals)
            .into_iter()
            .partition(|group| group.record_gone && group.pending_ids.is_empty());
        self.pending_folder_removals = waiting;
        for group in ready {
            self.dispatch_created_folder_removal(group.removal, group.claim, Resumed::No);
        }
    }

    /// Finish the folder removals an earlier run wrote down and never
    /// finished, each the way a delete runs it, with a status saying so.
    pub(crate) fn resume_pending_folder_removals(&mut self) {
        let rows = match self.session_store.load_pending_folder_removals() {
            Ok(rows) => rows,
            Err(err) => {
                crate::logger::error(&format!(
                    "could not read the folder deletions left from the last run: {err:#}"
                ));
                return;
            }
        };
        for row in rows {
            // Written in the transaction that deleted the record, so an agent
            // with this id is a different one, and its folder is not this
            // removal's to take.
            if self.sessions.iter().any(|s| s.id == row.session_id) {
                self.forget_pending_folder_removal(&row.session_id);
                continue;
            }
            let removal = CreatedFolderRemoval {
                session_id: row.session_id,
                path: PathBuf::from(row.path),
                identity: row.identity,
                label: row.label,
            };
            let claim = self
                .removal_coordination
                .ops
                .announce_removal(&removal.path);
            self.dispatch_created_folder_removal(removal, claim, Resumed::Yes);
        }
    }

    fn forget_pending_folder_removal(&self, session_id: &str) {
        if let Err(err) = self.session_store.delete_pending_folder_removal(session_id) {
            crate::logger::warn(&format!(
                "could not clear the folder deletion of agent {session_id}: {err:#}"
            ));
        }
    }

    /// Delete the folder on a worker, with a busy status and a final. A folder
    /// anything is found in, or that is no longer the one dux created, is kept,
    /// with a warning saying why, and dux lets go of a folder it created and
    /// keeps (see [`crate::agent_folders::let_go`]). Nothing is tried again.
    fn dispatch_created_folder_removal(
        &mut self,
        removal: CreatedFolderRemoval,
        claim: crate::worktree_ops::RemovalClaim,
        resumed: Resumed,
    ) {
        let op = folder_removal_op(&removal, resumed);
        let pending = self.begin_status_op(&op);
        let _ = self.worker_tx.send(WorkerEvent::PollerStatus(pending));
        // The PTYs dux runs and the creates under way are known only here.
        let occupant = self
            .folder_occupant(
                &removal.path,
                Some(&removal.session_id),
                crate::engine::StoppingProcesses::Occupy,
            )
            .map(|occupant| occupant.reason());
        let key = op.status_key().to_string();
        let tx = self.worker_tx.clone();
        let inputs = FolderRemovalInputs {
            claim,
            wait: self.removal_wait(),
            registry: self.process_registry.clone(),
            db_path: self.paths.sessions_db_path.clone(),
        };
        let CreatedFolderRemoval {
            session_id,
            path,
            identity,
            label,
        } = removal;
        let (unstarted, unstarted_path) = (session_id.clone(), path.clone());
        let spawned = super::spawn_worker::spawn_engine_thread("dux-folder-remove", move || {
            let db_path = inputs.db_path.clone();
            let joined = matches!(inputs.claim, crate::worktree_ops::RemovalClaim::Join(_));
            let result = match occupant {
                Some(reason) => Err(FolderKept::untouched(reason)),
                None => perform_created_folder_removal(&session_id, &path, identity, inputs),
            };
            if result.is_err() && !joined {
                crate::agent_folders::let_go(&path, identity.mark);
            }
            forget_pending_folder_removal_in(&db_path, &session_id);
            let _ = tx.send(WorkerEvent::StatusOpCompleted {
                resolved: op.resolve(&result),
            });
        });
        self.removal_workers.retain(|worker| !worker.is_finished());
        match spawned {
            Ok(handle) => self.removal_workers.push(handle),
            Err(err) => {
                crate::logger::error(&format!("could not start the folder removal: {err}"));
                self.forget_pending_folder_removal(&unstarted);
                let _ = self.worker_tx.send(WorkerEvent::StatusOpCompleted {
                    resolved: crate::engine::ResolvedFinal::new(
                        key,
                        kept_final(
                            &label,
                            &unstarted_path,
                            &FolderKept::untouched(format!(
                                "dux could not start the background work that deletes it ({err})"
                            )),
                        ),
                    ),
                });
            }
        }
    }
}

/// Whether a removal is finished at a start, after an earlier run of dux
/// quit before finishing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resumed {
    No,
    Yes,
}

/// The status op of one folder removal: its busy, and its final for each
/// outcome.
fn folder_removal_op(
    removal: &CreatedFolderRemoval,
    resumed: Resumed,
) -> crate::engine::StatusOp<crate::agent_folders::RemovedFolder, FolderKept> {
    let shown = crate::home_path::shorten_home(&removal.path);
    let label = removal.label.clone();
    let (success_label, success_path) = (label.clone(), shown.clone());
    let (failure_label, failure_path) = (label.clone(), removal.path.clone());
    let since = match resumed {
        Resumed::No => "",
        Resumed::Yes => ", which dux had not finished when it last quit",
    };
    crate::engine::status_op(crate::status_text![
        "Deleting the folder ",
        q(shown),
        " that dux created for deleted agent ",
        q(label),
        format!("{since}\u{2026}")
    ])
    .on_success(
        move |outcome: &crate::agent_folders::RemovedFolder| match outcome {
            crate::agent_folders::RemovedFolder::Removed => Final::info(crate::status_text![
                "Deleted the folder ",
                q(success_path.clone()),
                " that dux created for agent ",
                q(success_label.clone()),
                format!(", with everything in it{since}.")
            ]),
            crate::agent_folders::RemovedFolder::AlreadyGone => Final::info(crate::status_text![
                "The folder ",
                q(success_path.clone()),
                " of deleted agent ",
                q(success_label.clone()),
                " was already gone, so there was nothing to delete."
            ]),
        },
    )
    .on_failure(move |kept: &FolderKept| kept_final(&failure_label, &failure_path, kept))
}

/// The final for a folder that was kept, in whole or in part, saying why.
fn kept_final(label: &str, path: &Path, kept: &FolderKept) -> Final {
    let (opening, closing) = if kept.deleted_some {
        (
            "Deleted only part of the folder ",
            ". Delete the rest yourself if you no longer need it.",
        )
    } else {
        (
            "Kept the folder ",
            ". Delete it yourself if you no longer need it.",
        )
    };
    // STICKY: a folder the user asked to delete is still on disk, and only
    // they can deal with it.
    Final::warning(crate::status_text![
        opening,
        q(crate::home_path::shorten_home(path)),
        " that dux created for deleted agent ",
        q(label.to_string()),
        ": ",
        kept.reason.clone(),
        closing
    ])
    .sticky()
}

/// What the folder removal's worker takes from the engine.
struct FolderRemovalInputs {
    /// The claim taken when the delete began.
    claim: crate::worktree_ops::RemovalClaim,
    /// How long to wait for operations holding the folder.
    wait: std::time::Duration,
    /// Every session dux follows, the deleted agent's included.
    registry: crate::process_sessions::AgentProcessRegistry,
    /// The session database, for the last look over agents and projects.
    db_path: PathBuf,
}

/// The removal, on a worker thread. Nothing is ended, and the claim is
/// finished on every exit; a panic becomes an `Err`.
fn perform_created_folder_removal(
    session_id: &str,
    folder: &Path,
    identity: DirectoryIdentity,
    inputs: FolderRemovalInputs,
) -> Result<crate::agent_folders::RemovedFolder, FolderKept> {
    use std::panic::AssertUnwindSafe;
    let FolderRemovalInputs {
        claim,
        wait,
        registry,
        db_path,
    } = inputs;
    let folder_text = folder.to_string_lossy().into_owned();
    let nothing_ended = crate::engine::RemovalProcesses::none();
    let mut lease = None;
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let leading = match claim {
            crate::worktree_ops::RemovalClaim::Lead(leading) => lease.insert(leading),
            crate::worktree_ops::RemovalClaim::Join(_) => {
                return Err(FolderKept::untouched("dux is already deleting it"));
            }
        };
        if let Some(reason) = super::events::standalone_processes_keep_in(
            &registry,
            &folder_text,
            RemovalPlace::CreatedFolder,
        ) {
            return Err(reason.into());
        }
        if let Some(reason) = super::events::cwd_keeps_in(
            &registry,
            &folder_text,
            &nothing_ended,
            RemovalPlace::CreatedFolder,
        ) {
            return Err(reason.into());
        }
        let leading: &crate::worktree_ops::RemovalLease = leading;
        super::events::wait_then_last_look_in(
            leading,
            &folder_text,
            wait,
            |_| {},
            &[&registry],
            RemovalPlace::CreatedFolder,
            || {
                super::events::occupant_after_wait(
                    leading,
                    &registry,
                    &registry,
                    &nothing_ended,
                    Some((&db_path, Some(session_id))),
                )
                .or_else(|| program_working_in(folder))
            },
        )?;
        crate::agent_folders::remove_created_folder(folder, identity)
    }))
    .unwrap_or_else(|payload| {
        let reason = crate::engine::spawn_worker::format_panic_payload(&*payload);
        crate::logger::error(&format!(
            "the folder removal of deleted agent {session_id} panicked: {reason}"
        ));
        Err(FolderKept::untouched(format!(
            "the work that deletes it stopped unexpectedly ({reason})"
        )))
    });
    if let Some(lease) = lease {
        lease.finish(
            result
                .clone()
                .map(|_| crate::engine::RemovedBranches::Outcomes(Vec::new()))
                .map_err(|kept| kept.reason.message().to_string()),
        );
    }
    result
}

/// Any program of this user, dux-started or not, whose current folder is the
/// folder or one inside it, as the reason to keep it. A process list that
/// cannot be read keeps it too.
pub fn program_working_in(folder: &Path) -> Option<String> {
    let table = match crate::process_sessions::read_process_table_strict() {
        Ok(table) => table,
        Err(why) => {
            return Some(format!(
                "dux could not read the process list to check that no program is working in it \
                 ({why})"
            ));
        }
    };
    match crate::process_sessions::occupants_of(&table, &[folder.to_path_buf()]) {
        Ok(found) => found.first().map(|who| {
            format!(
                "{} (pid {}) is working in it; close it or make it leave that folder first",
                who.command, who.pid
            )
        }),
        Err(why) => Some(format!(
            "dux could not check which programs are working in it ({why})"
        )),
    }
}

/// Forget a written-down folder removal from a worker thread, on a connection
/// of its own. A failure is logged: the next start then runs the removal
/// again, which finds the folder gone or keeps it with the same reason.
fn forget_pending_folder_removal_in(db_path: &Path, session_id: &str) {
    let cleared = crate::storage::SessionStore::open_live(db_path)
        .and_then(|store| store.delete_pending_folder_removal(session_id));
    if let Err(err) = cleared {
        crate::logger::warn(&format!(
            "could not clear the finished folder deletion of agent {session_id}: {err:#}"
        ));
    }
}

use crate::engine::status_op::PendingStatusOp as _;
