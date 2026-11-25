//! Deleting the folder dux created for a standalone agent, once the agent has
//! been deleted and every process it ran there has gone.
//!
//! The delete itself removes the agent's record at once. The folder waits on a
//! barrier over the agent's tabs and terminals, which the reaper releases;
//! then a worker deletes it through [`crate::agent_folders::remove_created_folder`],
//! with a busy status and a final on the shared status lane, so both surfaces
//! hear the outcome.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::engine::{Engine, Final};
use crate::model::DirectoryIdentity;
use crate::status_text::StatusText;
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

/// A folder removal waiting for the PTYs in `pending_ids` to reap.
#[derive(Debug)]
pub struct GroupFolderRemoval {
    /// The tabs and terminals of the deleted agent still terminating.
    pub pending_ids: HashSet<String>,
    /// The removal to run once they are all gone.
    pub removal: CreatedFolderRemoval,
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

    /// Wait for the PTYs of `pty_ids` still terminating, then delete the
    /// folder; with none left, delete it now.
    pub(crate) fn park_created_folder_removal(
        &mut self,
        removal: CreatedFolderRemoval,
        pty_ids: &HashSet<String>,
    ) {
        let pending_ids: HashSet<String> = self
            .terminating_ptys
            .iter()
            .filter(|entry| pty_ids.contains(&entry.id))
            .map(|entry| entry.id.clone())
            .collect();
        if pending_ids.is_empty() {
            self.dispatch_created_folder_removal(removal);
        } else {
            self.pending_folder_removals.push(GroupFolderRemoval {
                pending_ids,
                removal,
            });
        }
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
            .partition(|group| group.pending_ids.is_empty());
        self.pending_folder_removals = waiting;
        for group in ready {
            self.dispatch_created_folder_removal(group.removal);
        }
    }

    /// Every other agent's folder and every project's path, each with the
    /// words that name it in the status, for the worker's occupant check.
    fn created_folder_candidates(&self, deleted: &str) -> Vec<(PathBuf, StatusText)> {
        let agents = self
            .sessions
            .iter()
            .filter(|session| session.id != deleted)
            .map(|session| {
                (
                    PathBuf::from(session.directory()),
                    crate::status_text!["agent ", q(session.display_label()), " now works in it"],
                )
            });
        let projects = self.projects.iter().map(|project| {
            (
                PathBuf::from(&project.path),
                crate::status_text!["project ", q(project.name.clone()), " is in it"],
            )
        });
        agents.chain(projects).collect()
    }

    /// Delete the folder on a worker, with a busy status and a final. A folder
    /// another agent or a project has come to use meanwhile is kept, with a
    /// warning saying why.
    pub(crate) fn dispatch_created_folder_removal(&mut self, removal: CreatedFolderRemoval) {
        let shown = crate::home_path::shorten_home(&removal.path);
        let label = removal.label.clone();
        let candidates = self.created_folder_candidates(&removal.session_id);
        let (success_label, success_path) = (label.clone(), shown.clone());
        let (failure_label, failure_path) = (label.clone(), shown.clone());
        let op = crate::engine::status_op(crate::status_text![
            "Deleting the folder ",
            q(shown),
            " that dux created for deleted agent ",
            q(label),
            "\u{2026}"
        ])
        .on_success(move |outcome: &FolderRemovalOutcome| match outcome {
            FolderRemovalOutcome::Removed => Final::info(crate::status_text![
                "Deleted the folder ",
                q(success_path.clone()),
                " that dux created for agent ",
                q(success_label.clone()),
                ", with everything in it."
            ]),
            FolderRemovalOutcome::AlreadyGone => Final::info(crate::status_text![
                "The folder ",
                q(success_path.clone()),
                " of deleted agent ",
                q(success_label.clone()),
                " was already gone, so there was nothing to delete."
            ]),
            FolderRemovalOutcome::Occupied(occupant) => Final::warning(crate::status_text![
                "Kept the folder ",
                q(success_path.clone()),
                " of deleted agent ",
                q(success_label.clone()),
                ", because ",
                occupant.clone(),
                "."
            ]),
        })
        .on_failure(move |reason: &String| {
            // STICKY: a folder is left on disk that the user has to deal with.
            Final::warning(crate::status_text![
                "Kept the folder ",
                q(failure_path.clone()),
                " of deleted agent ",
                q(failure_label.clone()),
                format!(": {reason}. Delete it yourself if you no longer need it.")
            ])
            .sticky()
        });
        let pending = self.begin_status_op(&op);
        let _ = self.worker_tx.send(WorkerEvent::PollerStatus(pending));
        let key = op.status_key().to_string();
        let tx = self.worker_tx.clone();
        let CreatedFolderRemoval { path, identity, .. } = removal;
        let spawned = super::spawn_worker::spawn_engine_thread("dux-folder-remove", move || {
            let result =
                match created_folder_occupant(&path, &candidates) {
                    Some(occupant) => Ok(FolderRemovalOutcome::Occupied(occupant)),
                    None => crate::agent_folders::remove_created_folder(&path, identity).map(
                        |removed| match removed {
                            crate::agent_folders::RemovedFolder::Removed => {
                                FolderRemovalOutcome::Removed
                            }
                            crate::agent_folders::RemovedFolder::AlreadyGone => {
                                FolderRemovalOutcome::AlreadyGone
                            }
                        },
                    ),
                };
            let _ = tx.send(WorkerEvent::StatusOpCompleted {
                resolved: op.resolve(&result),
            });
        });
        self.removal_workers.retain(|worker| !worker.is_finished());
        match spawned {
            Ok(handle) => self.removal_workers.push(handle),
            Err(err) => {
                crate::logger::error(&format!("could not start the folder removal: {err}"));
                let _ = self.worker_tx.send(WorkerEvent::StatusOpCompleted {
                    resolved: crate::engine::ResolvedFinal::new(
                        key,
                        Final::warning(crate::status_text![
                            "Kept the folder ",
                            q(shown),
                            " of deleted agent ",
                            q(label),
                            format!(
                                ": dux could not start the background work that deletes it \
                                 ({err}). Delete it yourself if you no longer need it."
                            )
                        ])
                        .sticky(),
                    ),
                });
            }
        }
    }
}

/// What the folder removal's worker did.
#[derive(Clone, Debug)]
enum FolderRemovalOutcome {
    /// The folder and everything in it were deleted.
    Removed,
    /// Nothing was at the folder's path any more.
    AlreadyGone,
    /// Another agent or a project is in the folder, so it was kept; the text
    /// names which.
    Occupied(StatusText),
}

/// The first of `candidates` whose path is `folder` or lies inside it, both
/// compared with every symbolic link resolved, so a path that reaches the
/// folder through a link still counts. A candidate whose path no longer
/// resolves is compared as written.
fn created_folder_occupant(
    folder: &Path,
    candidates: &[(PathBuf, StatusText)],
) -> Option<StatusText> {
    let resolve = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let folder = resolve(folder);
    candidates
        .iter()
        .find(|(path, _)| resolve(path).starts_with(&folder))
        .map(|(_, occupant)| occupant.clone())
}

use crate::engine::status_op::PendingStatusOp as _;
