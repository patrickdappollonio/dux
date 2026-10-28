//! Worktree folders a delete left behind, and the worktree manager's
//! confirmed delete of one.
//!
//! When something keeps writing into a worktree while git removes it, git lets
//! go of the worktree and cannot delete the folder (see
//! [`git::LeftoverWorktreeFolder`]). The folder is then invisible to `git
//! worktree list`, so the removal records it in the session database, with the
//! branches it kept because of it (each at the commit it pointed at) and the
//! folder's identity (device, inode and, where the filesystem keeps one, birth
//! time), and both worktree managers list it as a row of its own.
//!
//! dux never deletes such a folder on its own. The user may, from the manager,
//! behind a confirmation that names it, and only while every check below still
//! holds right before the delete: the record names exactly this folder, it is
//! inside dux's worktrees root for the project with no symbolic link on the
//! way, git lists no worktree at or inside it, it holds no other repository,
//! and nothing occupies it.
//!
//! A folder that is gone (deleted by hand, or replaced by a different folder
//! or a link) is never deleted. Its record stays listed while a branch it kept
//! is still there, so that branch can still be deleted from the manager, and
//! is forgotten once none is.

use std::path::{Path, PathBuf};

use crate::config::DuxPaths;
use crate::git::{self, KeptBranch, KeptBranchDeletion};
use crate::model::{BranchProvenance, Project};
use crate::prose::Prose;
use crate::worktree_manager::BusyFolder;
use crate::worktree_ops::WorktreeOps;

/// Which folder a record is about: a path names whatever is there now, and a
/// new folder at the same path is not the one that was left behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderIdentity {
    /// The device number.
    pub dev: u64,
    /// The inode number.
    pub ino: u64,
    /// The birth time in nanoseconds since the epoch, where the filesystem
    /// keeps one: an inode number can be reused by a folder made later.
    pub birth_ns: Option<i64>,
}

impl FolderIdentity {
    /// Whether `now` is the folder this identity was recorded for. A birth
    /// time recorded must still be there and equal; none recorded leaves the
    /// device and inode to decide.
    fn is(&self, now: &FolderIdentity) -> bool {
        self.dev == now.dev
            && self.ino == now.ino
            && self
                .birth_ns
                .is_none_or(|birth| now.birth_ns == Some(birth))
    }
}

/// A left-over folder as the session database keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftoverFolderRecord {
    /// The canonical path of the folder at the failure.
    pub path: String,
    /// The project whose worktree it was.
    pub project_id: String,
    /// The branches kept because the folder was left behind, each at the
    /// commit it pointed at then (none when git could not be asked).
    pub branches: Vec<KeptBranch>,
    /// The agent's branch provenance; `None` when the worktree manager's own
    /// removal left the folder (there was no agent).
    pub provenance: Option<BranchProvenance>,
    /// The folder's identity.
    pub identity: FolderIdentity,
}

/// The folder's identity, `None` when the path is not a real directory (gone,
/// a symbolic link, a file). Not followed through a link at the path itself.
fn folder_identity(path: &Path) -> std::io::Result<Option<FolderIdentity>> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(Some(FolderIdentity {
            dev: meta.dev(),
            ino: meta.ino(),
            birth_ns: meta
                .created()
                .ok()
                .and_then(|born| born.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|since| i64::try_from(since.as_nanos()).ok()),
        })),
        Ok(_) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// What is at a record's path now.
enum FolderState {
    /// The folder that was left behind.
    Present,
    /// Nothing, or something else: a different folder, a link, a file.
    Gone,
    /// The path could not be read.
    Unreadable(String),
}

fn folder_state(record: &LeftoverFolderRecord) -> FolderState {
    match folder_identity(Path::new(&record.path)) {
        Ok(Some(now)) if record.identity.is(&now) => FolderState::Present,
        Ok(_) => FolderState::Gone,
        Err(err) => FolderState::Unreadable(err.to_string()),
    }
}

/// Record the folder `leftover` names for `project_id`'s worktree manager, and
/// mark the error as listed there when the record was written, so its message
/// sends the user to the manager only when the manager will show it. Nothing
/// is written (and the message keeps sending the user to delete it by hand)
/// when the project is gone, the folder is not a real directory, its path is
/// not text, or the database cannot be written. A worker thread's call.
pub(crate) fn record_leftover(
    db_path: &Path,
    repo: &Path,
    project_id: &str,
    provenance: Option<BranchProvenance>,
    leftover: &mut git::LeftoverWorktreeFolder,
) {
    let written = (|| -> anyhow::Result<bool> {
        let canonical = std::fs::canonicalize(&leftover.path)?;
        let Some(identity) = folder_identity(&canonical)? else {
            return Ok(false);
        };
        let Some(path) = canonical.to_str() else {
            return Ok(false);
        };
        let store = crate::storage::SessionStore::open_live(db_path)?;
        let mut branches: Vec<KeptBranch> = leftover
            .kept_branches
            .iter()
            .map(|kept| KeptBranch {
                provenance,
                ..kept.clone()
            })
            .collect();
        // A path recorded before keeps the branches of that record that are
        // still there, each with its own commit and provenance: the new
        // record must not make them invisible.
        if let Some(earlier) = store
            .load_leftover_folders(project_id)?
            .into_iter()
            .find(|record| record.path == path)
        {
            for kept in earlier.branches {
                let still_there = !matches!(git::resolve_branch_oid(repo, &kept.name), Ok(None));
                if still_there && !branches.iter().any(|branch| branch.name == kept.name) {
                    branches.push(kept);
                }
            }
        }
        store.record_leftover_folder(&LeftoverFolderRecord {
            path: path.to_string(),
            project_id: project_id.to_string(),
            branches,
            provenance,
            identity,
        })
    })();
    leftover.in_manager = match written {
        Ok(written) => written,
        Err(err) => {
            crate::logger::warn(&format!(
                "could not record the left-over folder {} for the worktree manager: {err:#}",
                leftover.path.display()
            ));
            false
        }
    };
}

/// [`record_leftover`] for whatever a removal returned: a no-op unless it is a
/// left-over folder.
pub(crate) fn record_if_leftover(
    db_path: &Path,
    repo: &Path,
    project_id: &str,
    provenance: Option<BranchProvenance>,
    mut err: anyhow::Error,
) -> anyhow::Error {
    if let Some(leftover) = err.downcast_mut::<git::LeftoverWorktreeFolder>() {
        record_leftover(db_path, repo, project_id, provenance, leftover);
    }
    err
}

/// One left-over folder row of the worktree manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftoverFolder {
    /// The folder.
    pub path: PathBuf,
    /// Whether the folder is still there. `false` when it was deleted by hand
    /// or replaced by something else, while a branch it kept is still there:
    /// the row is then about that branch, and nothing at the path is deleted.
    pub folder_present: bool,
    /// The kept branches a delete can remove, by name: each still points
    /// where it did and no other worktree has it checked out.
    pub branches: Vec<String>,
    /// The kept branches a delete would keep, each with why.
    pub undeletable: Vec<UndeletableBranch>,
    /// The provenance the dialog warns by and defaults its box from: the
    /// first deletable branch's that is not dux's, else the record's. `None`
    /// when the manager's own removal kept them.
    pub provenance: Option<BranchProvenance>,
    /// What left the folder behind.
    pub left_by: LeftBy,
    /// What is using the folder now, when something is; such a row offers no
    /// delete.
    pub busy: Option<String>,
}

/// A kept branch that a delete would not remove, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndeletableBranch {
    /// The branch.
    pub name: String,
    /// Why it is kept.
    pub why: BranchBlock,
}

/// Why dux will not delete a kept branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchBlock {
    /// No commit was recorded for it, so whether it changed cannot be told.
    NoCommit,
    /// Another worktree has it checked out (the path as shown).
    CheckedOut {
        /// Where.
        path: String,
    },
    /// It points somewhere else than when the folder was left behind.
    Moved,
    /// git could not be asked about it.
    Unchecked {
        /// git's error.
        reason: String,
    },
}

impl LeftoverFolder {
    /// Whether the delete dialog ticks "also delete the branch" to begin
    /// with: for a branch dux created, and for one the user had already asked
    /// the manager to delete; never for one that predates its agent.
    pub fn deletes_branches_by_default(&self) -> bool {
        self.provenance
            .is_none_or(|provenance| provenance.dux_may_delete_branch())
    }

    /// Whether the row offers a delete at all: there is a folder to delete,
    /// or a kept branch a delete can remove.
    pub fn offers_delete(&self) -> bool {
        self.folder_present || !self.branches.is_empty()
    }
}

/// What listing makes of one kept branch: gone, deletable, or kept with why.
enum KeptState {
    Gone,
    Deletable,
    Kept(BranchBlock),
}

fn kept_state(repo: &Path, kept: &KeptBranch) -> KeptState {
    let now = match git::resolve_branch_oid(repo, &kept.name) {
        Ok(None) => return KeptState::Gone,
        Ok(Some(now)) => now,
        Err(err) => {
            return KeptState::Kept(BranchBlock::Unchecked {
                reason: format!("{err:#}"),
            });
        }
    };
    match kept.oid.as_deref() {
        None => return KeptState::Kept(BranchBlock::NoCommit),
        Some(recorded) if recorded != now => return KeptState::Kept(BranchBlock::Moved),
        Some(_) => {}
    }
    match git::branch_checked_out_in(repo, &kept.name) {
        Ok(None) => KeptState::Deletable,
        Ok(Some(path)) => KeptState::Kept(BranchBlock::CheckedOut {
            path: crate::home_path::shorten_home(&path),
        }),
        Err(err) => KeptState::Kept(BranchBlock::Unchecked {
            reason: format!("{err:#}"),
        }),
    }
}

/// The left-over folders recorded for `project`, for its worktree manager.
/// A record whose folder is a registered worktree again is forgotten; so is
/// one whose folder is gone (or replaced) once none of its branches remain.
/// Reads the database and runs git: worker threads only. A database that
/// cannot be read lists none (the worktree rows are still useful) and is
/// logged.
pub fn list_leftover_folders(
    project: &Project,
    paths: &DuxPaths,
    ops: &WorktreeOps,
    busy: &[BusyFolder],
) -> Vec<LeftoverFolder> {
    let store = match crate::storage::SessionStore::open_live(&paths.sessions_db_path) {
        Ok(store) => store,
        Err(err) => {
            crate::logger::warn(&format!("could not read the left-over folders: {err:#}"));
            return Vec::new();
        }
    };
    let records = match store.load_leftover_folders(&project.id) {
        Ok(records) => records,
        Err(err) => {
            crate::logger::warn(&format!("could not read the left-over folders: {err:#}"));
            return Vec::new();
        }
    };
    let repo = Path::new(&project.path);
    records
        .into_iter()
        .filter_map(|record| {
            let path = PathBuf::from(&record.path);
            let folder_present = match folder_state(&record) {
                // Unreadable is not gone: listed, and the delete decides.
                FolderState::Present | FolderState::Unreadable(_) => true,
                FolderState::Gone => false,
            };
            let mut deletable = Vec::new();
            let mut undeletable = Vec::new();
            for kept in &record.branches {
                match kept_state(repo, kept) {
                    KeptState::Gone => {}
                    KeptState::Deletable => deletable.push(kept),
                    KeptState::Kept(why) => undeletable.push(UndeletableBranch {
                        name: kept.name.clone(),
                        why,
                    }),
                }
            }
            // A record lives as long as there is something left to do with
            // it: its folder, or a kept branch that still exists.
            let forget = if folder_present {
                git::worktree_registration_checked(repo, &path).unwrap_or(false)
            } else {
                deletable.is_empty() && undeletable.is_empty()
            };
            if forget {
                if let Err(err) = store.delete_leftover_folder(&record.path, &record.identity) {
                    crate::logger::warn(&format!(
                        "could not forget the left-over folder {}: {err:#}",
                        record.path
                    ));
                }
                return None;
            }
            let busy = if ops.is_being_removed(&path) {
                Some("it is being deleted".to_string())
            } else if ops
                .holders(&path)
                .contains(&crate::worktree_ops::WorktreeOpKind::CreateAgent)
            {
                Some("an agent is being created in it".to_string())
            } else {
                crate::worktree_manager::in_use_by(&path, None, busy)
            };
            let provenance = deletable
                .iter()
                .map(|kept| kept.provenance)
                .find(|provenance| !provenance.is_none_or(|p| p.dux_may_delete_branch()))
                .unwrap_or(record.provenance);
            Some(LeftoverFolder {
                path,
                folder_present,
                branches: deletable.iter().map(|kept| kept.name.clone()).collect(),
                undeletable,
                provenance,
                left_by: if record.provenance.is_some() {
                    LeftBy::Agent
                } else {
                    LeftBy::Manager
                },
                busy,
            })
        })
        .collect()
}

/// What happened to one branch kept with a left-over folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchFate {
    /// Deleted, as asked.
    Deleted,
    /// Kept, as the user chose; it is still there.
    Kept,
    /// Gone before dux got to it.
    AlreadyGone,
    /// Kept because it points somewhere else than when the folder was left
    /// behind.
    Moved,
    /// git refused; the branch is still there.
    Refused {
        /// Why.
        reason: String,
    },
    /// git could not be asked about it, so it was kept.
    CheckFailed {
        /// git's error.
        reason: String,
    },
    /// Kept because no commit was recorded for it, so whether it moved since
    /// cannot be told.
    Unverifiable,
}

/// One branch and what happened to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftoverBranchOutcome {
    /// The branch.
    pub name: String,
    /// What happened to it.
    pub fate: BranchFate,
}

/// What a Delete folder request came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeftoverRemovalOutcome {
    /// Not inside dux's worktrees root for the project.
    NotManaged,
    /// The folder holds the project's own repository.
    HoldsProjectCheckout,
    /// dux has no record of a left-over folder there.
    NotLeftover,
    /// The path could not be read.
    Unreadable {
        /// The filesystem's error.
        reason: String,
    },
    /// The path is reached through a symbolic link.
    Symlink,
    /// git could not say whether the path is, or holds, a worktree.
    GitUnknown {
        /// git's error.
        reason: String,
    },
    /// git lists the path as a worktree again.
    Registered,
    /// A worktree of the repository, or another repository, is inside it.
    HoldsCheckout {
        /// Where.
        path: PathBuf,
    },
    /// A subfolder could not be read, so whether a checkout is in it cannot
    /// be told.
    Unlookable {
        /// The subfolder.
        path: PathBuf,
        /// The filesystem's error.
        reason: String,
    },
    /// A removal of the folder is already running.
    BeingRemoved,
    /// Something is using the folder.
    Busy {
        /// What.
        reason: String,
    },
    /// The delete failed part-way; what is left. The record stays.
    StillThere {
        /// Up to a handful of what is left, relative to the folder.
        leftovers: Vec<String>,
        /// How many more.
        more: usize,
        /// The filesystem's error.
        error: String,
    },
    /// The folder was deleted.
    Removed {
        /// What happened to each recorded branch.
        branches: Vec<LeftoverBranchOutcome>,
    },
    /// The folder was already gone (or replaced, and left alone); only the
    /// branches were acted on.
    FolderAlreadyGone {
        /// What happened to each recorded branch.
        branches: Vec<LeftoverBranchOutcome>,
    },
}

/// A Delete folder request the engine admitted against live state.
/// [`Self::run`] does the rest on a worker thread.
#[derive(Debug)]
pub struct AdmittedLeftoverRemoval {
    pub(crate) ops: WorktreeOps,
    pub(crate) project: Project,
    pub(crate) paths: DuxPaths,
    pub(crate) requested: PathBuf,
    /// The branches the user ticked, by name; only those the record kept are
    /// ever deleted.
    pub(crate) delete_branches: Vec<String>,
    pub(crate) wait: std::time::Duration,
    pub(crate) registry: crate::process_sessions::AgentProcessRegistry,
}

/// Whether `path` is strictly inside `root` and no component from `root` down
/// to it is a symbolic link. `root` must be canonical.
fn contained_without_links(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return false;
    }
    let mut walked = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(part) = component else {
            return false;
        };
        walked.push(part);
        match std::fs::symlink_metadata(&walked) {
            Ok(meta) if !meta.file_type().is_symlink() => {}
            _ => return false,
        }
    }
    true
}

/// A repository inside `folder`: a `.git` entry below its top level (a
/// worktree or clone of anything), or a `.git` DIRECTORY at it (a whole
/// repository; a worktree's own `.git` is a file, and git no longer lists this
/// one). Walked without following links.
/// A subfolder the walk could not read is an answer it cannot give, so it
/// refuses rather than skips it.
fn repository_inside(folder: &Path) -> Option<LeftoverRemovalOutcome> {
    for entry in walkdir::WalkDir::new(folder)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                return Some(LeftoverRemovalOutcome::Unlookable {
                    path: err
                        .path()
                        .map_or_else(|| folder.to_path_buf(), Path::to_path_buf),
                    reason: err
                        .io_error()
                        .map_or_else(|| err.to_string(), ToString::to_string),
                });
            }
        };
        if entry.file_name() == ".git" && (entry.depth() > 1 || entry.file_type().is_dir()) {
            return Some(LeftoverRemovalOutcome::HoldsCheckout {
                path: entry
                    .path()
                    .parent()
                    .map_or_else(|| folder.to_path_buf(), Path::to_path_buf),
            });
        }
    }
    None
}

impl AdmittedLeftoverRemoval {
    /// Every check that guards deleting a folder that is there, against the
    /// disk and git as they are now. `None` when it may go.
    fn refusal(&self, record: &LeftoverFolderRecord) -> Option<LeftoverRemovalOutcome> {
        let path = Path::new(&record.path);
        let Ok(root) = std::fs::canonicalize(self.paths.worktrees_root.join(&self.project.name))
        else {
            return Some(LeftoverRemovalOutcome::NotManaged);
        };
        match std::fs::canonicalize(path) {
            Ok(canonical) if canonical == path => {}
            _ => return Some(LeftoverRemovalOutcome::Symlink),
        }
        if !path.starts_with(&root) {
            return Some(LeftoverRemovalOutcome::NotManaged);
        }
        if !contained_without_links(&root, path) {
            return Some(LeftoverRemovalOutcome::Symlink);
        }
        let worktrees = match git::list_worktrees(Path::new(&self.project.path)) {
            Ok(worktrees) => worktrees,
            Err(err) => {
                return Some(LeftoverRemovalOutcome::GitUnknown {
                    reason: format!("{err:#}"),
                });
            }
        };
        let folder_key = crate::worktree_ops::path_key(path);
        for worktree in worktrees {
            if crate::worktree_ops::path_key(&worktree.path) == folder_key {
                return Some(LeftoverRemovalOutcome::Registered);
            }
            if crate::worktree_ops::folder_contains(path, &worktree.path) {
                return Some(LeftoverRemovalOutcome::HoldsCheckout {
                    path: worktree.path,
                });
            }
        }
        repository_inside(path)
    }

    /// The record for the requested path, `None` when there is none.
    fn record(
        &self,
        store: &crate::storage::SessionStore,
    ) -> Result<Option<LeftoverFolderRecord>, String> {
        let Some(requested) = self.requested.to_str() else {
            return Ok(None);
        };
        store
            .load_leftover_folders(&self.project.id)
            .map(|records| records.into_iter().find(|record| record.path == requested))
            .map_err(|e| format!("dux could not read its left-over folders ({e:#})"))
    }

    /// Check, claim, wait, check again, delete. Blocking: worker threads only.
    ///
    /// The checks run again under the claim immediately before the delete.
    /// `remove_dir_all` walks the tree relative to the folder it opened, so
    /// what is left is an ancestor swapped for a link between the last check
    /// and that open, which takes a process of this same user acting against
    /// dux: outside the single-tenant trust model, and accepted.
    ///
    /// # Errors
    ///
    /// Fails with the reason when the session database cannot be read.
    pub fn run(self) -> Result<LeftoverRemovalOutcome, String> {
        let db_path = self.paths.sessions_db_path.clone();
        let store = crate::storage::SessionStore::open_live(&db_path)
            .map_err(|e| format!("dux could not read its left-over folders ({e:#})"))?;
        let Some(record) = self.record(&store)? else {
            return Ok(LeftoverRemovalOutcome::NotLeftover);
        };
        let settle = |branches: &[LeftoverBranchOutcome]| settle(&store, &record, branches);
        match folder_state(&record) {
            FolderState::Present => {}
            FolderState::Gone => {
                let branches = self.branch_fates(&record);
                settle(&branches);
                return Ok(LeftoverRemovalOutcome::FolderAlreadyGone { branches });
            }
            FolderState::Unreadable(reason) => {
                return Ok(LeftoverRemovalOutcome::Unreadable { reason });
            }
        }
        if let Some(refused) = self.refusal(&record) {
            return Ok(refused);
        }
        let lease = match self.ops.announce_removal(&record.path) {
            crate::worktree_ops::RemovalClaim::Lead(lease) => lease,
            crate::worktree_ops::RemovalClaim::Join(_) => {
                return Ok(LeftoverRemovalOutcome::BeingRemoved);
            }
        };
        let none = crate::engine::RemovalProcesses::none();
        let looked = crate::engine::wait_then_last_look(
            &lease,
            &record.path,
            self.wait,
            |_| {},
            &[&self.registry],
            || {
                crate::engine::occupant_after_wait(
                    &lease,
                    &self.registry,
                    &self.registry,
                    &none,
                    Some((&db_path, None)),
                )
            },
        );
        if let Err(reason) = looked {
            lease.finish(Err(reason.clone()));
            return Ok(LeftoverRemovalOutcome::Busy { reason });
        }
        // Everything again, immediately before the delete.
        let refused = match folder_state(&record) {
            FolderState::Present => self.refusal(&record),
            FolderState::Gone => {
                lease.finish(Err("the folder was already gone".to_string()));
                let branches = self.branch_fates(&record);
                settle(&branches);
                return Ok(LeftoverRemovalOutcome::FolderAlreadyGone { branches });
            }
            FolderState::Unreadable(reason) => Some(LeftoverRemovalOutcome::Unreadable { reason }),
        };
        if let Some(refused) = refused {
            lease.finish(Err(leftover_removal_report(&record.path, &refused)
                .message
                .to_string()));
            return Ok(refused);
        }
        if let Err(error) = std::fs::remove_dir_all(&record.path) {
            let (leftovers, more) =
                git::list_leftovers(Path::new(&record.path), git::LEFTOVERS_NAMED);
            lease.finish(Err(error.to_string()));
            return Ok(LeftoverRemovalOutcome::StillThere {
                leftovers,
                more,
                error: error.to_string(),
            });
        }
        let branches = self.branch_fates(&record);
        settle(&branches);
        lease.finish(Ok(crate::engine::RemovedBranches::Kept(
            crate::model::BranchKeptReason::UserDeclined,
        )));
        Ok(LeftoverRemovalOutcome::Removed { branches })
    }

    /// Delete the ticked branches the record kept, each only while it still
    /// points where it did, and say what became of every one of them. A git
    /// that cannot answer is never read as "gone".
    fn branch_fates(&self, record: &LeftoverFolderRecord) -> Vec<LeftoverBranchOutcome> {
        let repo = Path::new(&self.project.path);
        record
            .branches
            .iter()
            .map(|kept| {
                let fate = if self.delete_branches.contains(&kept.name) {
                    match git::delete_kept_branch(repo, kept) {
                        Ok(KeptBranchDeletion::Deleted) => BranchFate::Deleted,
                        Ok(KeptBranchDeletion::AlreadyGone) => BranchFate::AlreadyGone,
                        Ok(KeptBranchDeletion::Moved) => BranchFate::Moved,
                        Ok(KeptBranchDeletion::Refused { reason }) => {
                            BranchFate::Refused { reason }
                        }
                        Ok(KeptBranchDeletion::Unverifiable) => BranchFate::Unverifiable,
                        Err(err) => BranchFate::CheckFailed {
                            reason: format!("{err:#}"),
                        },
                    }
                } else {
                    match git::resolve_branch_oid(repo, &kept.name) {
                        Ok(Some(_)) => BranchFate::Kept,
                        Ok(None) => BranchFate::AlreadyGone,
                        Err(err) => BranchFate::CheckFailed {
                            reason: format!("{err:#}"),
                        },
                    }
                };
                LeftoverBranchOutcome {
                    name: kept.name.clone(),
                    fate,
                }
            })
            .collect()
    }
}

/// What a delete leaves of a record: it lives on while a kept branch still
/// exists, since there is still something to do with it, holding only the
/// branches that may still be there; with none, it is forgotten.
fn settle(
    store: &crate::storage::SessionStore,
    record: &LeftoverFolderRecord,
    branches: &[LeftoverBranchOutcome],
) {
    let remaining: Vec<KeptBranch> = record
        .branches
        .iter()
        .filter(|kept| {
            branches.iter().any(|branch| {
                branch.name == kept.name
                    && !matches!(branch.fate, BranchFate::Deleted | BranchFate::AlreadyGone)
            })
        })
        .cloned()
        .collect();
    let written = if remaining.is_empty() {
        store.delete_leftover_folder(&record.path, &record.identity)
    } else {
        store.keep_only_leftover_branches(&record.path, &record.identity, &remaining)
    };
    if let Err(err) = written {
        crate::logger::warn(&format!(
            "could not update the left-over folder {}: {err:#}",
            record.path
        ));
    }
}

/// What the engine answered a Delete folder request with.
#[derive(Debug)]
pub enum LeftoverAdmission {
    /// Refused at once, against live state.
    Refused(LeftoverRemovalOutcome),
    /// Admitted: the delete goes ahead on a worker.
    Admitted(Box<AdmittedLeftoverRemoval>),
}

/// `a`, `a and b`, `a, b and c`, each a name.
fn push_names(text: &mut crate::status_text::StatusText, names: &[&str]) {
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            text.push(if index + 1 == names.len() {
                " and "
            } else {
                ", "
            });
        }
        text.push_name(name);
    }
}

/// [`push_names`] for a dialog's prose.
fn prose_names(prose: &mut Prose, names: &[&str]) {
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            prose.push_text(if index + 1 == names.len() {
                " and "
            } else {
                ", "
            });
        }
        prose.push_name(*name);
    }
}

/// Whether a left-over folder is still on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderOnDisk {
    /// The folder that was left behind is there.
    Present,
    /// It was deleted by hand, or something else took its place.
    Gone,
}

/// What left a folder behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftBy {
    /// An agent's delete.
    Agent,
    /// The worktree manager's own removal of a worktree no agent held.
    Manager,
}

/// What the dialog's box says to do with the kept branches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchChoice {
    /// Delete them.
    Delete,
    /// Keep them.
    Keep,
}

impl LeftoverFolder {
    /// Whether the folder is still on disk.
    pub fn on_disk(&self) -> FolderOnDisk {
        if self.folder_present {
            FolderOnDisk::Present
        } else {
            FolderOnDisk::Gone
        }
    }
}

/// The Delete folder dialog's title, on both surfaces.
pub fn delete_dialog_title(folder: FolderOnDisk, branch_count: usize) -> &'static str {
    match (folder, branch_count) {
        (FolderOnDisk::Present, _) => "Delete the left-over folder?",
        (FolderOnDisk::Gone, 1) => "Delete the kept branch?",
        (FolderOnDisk::Gone, _) => "Delete the kept branches?",
    }
}

/// The Delete folder dialog's body, on both surfaces: what is deleted from
/// disk, and why it was left there.
pub fn delete_dialog_body(path: &str, folder: FolderOnDisk, left_by: LeftBy) -> Prose {
    match (folder, left_by) {
        (FolderOnDisk::Gone, _) => Prose::new()
            .text("The left-over folder at ")
            .name(path)
            .text(" is already gone, so nothing is deleted from disk."),
        (FolderOnDisk::Present, LeftBy::Agent) => Prose::new().name(path).text(
            " and everything in it will be deleted from disk. Something was still writing into \
             it when its agent was deleted; stop it first, or the delete fails again. This \
             cannot be undone: dux has no trash.",
        ),
        (FolderOnDisk::Present, LeftBy::Manager) => Prose::new().name(path).text(
            " and everything in it will be deleted from disk. Something was still writing into \
             it when its worktree was removed; stop it first, or the delete fails again. This \
             cannot be undone: dux has no trash.",
        ),
    }
}

/// What the dialog says becomes of the kept branches, following its box.
pub fn delete_dialog_branch_fate(
    branches: &[&str],
    choice: BranchChoice,
    folder: FolderOnDisk,
) -> Prose {
    let one = branches.len() == 1;
    let mut prose = Prose::new().text(if one { "The branch " } else { "The branches " });
    prose_names(&mut prose, branches);
    prose.text(match (choice, folder, one) {
        (BranchChoice::Keep, _, true) => " is kept.",
        (BranchChoice::Keep, _, false) => " are kept.",
        (BranchChoice::Delete, FolderOnDisk::Present, true) => {
            " will be deleted with it, unless it has moved since the folder was left behind."
        }
        (BranchChoice::Delete, FolderOnDisk::Present, false) => {
            " will be deleted with it, unless they have moved since the folder was left behind."
        }
        (BranchChoice::Delete, FolderOnDisk::Gone, true) => {
            " will be deleted, unless it has moved since the folder was left behind."
        }
        (BranchChoice::Delete, FolderOnDisk::Gone, false) => {
            " will be deleted, unless they have moved since the folder was left behind."
        }
    })
}

/// What a dialog or row says about a kept branch dux will not delete: why,
/// and how to delete it yourself. On both surfaces.
pub fn kept_branch_sentence(name: &str, why: &BranchBlock) -> Prose {
    let command = git::branch_delete_command(name);
    let prose = Prose::new()
        .text("The branch ")
        .name(name)
        .text(" is kept: ");
    match why {
        BranchBlock::NoCommit => prose
            .text(
                "dux has no record of the commit it was on when the folder was left behind. \
                 Delete it yourself with ",
            )
            .name(command)
            .text("."),
        BranchBlock::CheckedOut { path } => prose
            .text("it is checked out in ")
            .name(path)
            .text(". Delete it yourself with ")
            .name(command)
            .text(" once it is not."),
        BranchBlock::Moved => prose
            .text("it has moved since the folder was left behind. Delete it yourself with ")
            .name(command)
            .text("."),
        BranchBlock::Unchecked { reason } => prose
            .text(format!(
                "git could not check it ({reason}). Delete it yourself with "
            ))
            .name(command)
            .text("."),
    }
}

/// What a row says when something is using its folder, on both surfaces.
pub fn in_use_sentence(reason: &str) -> String {
    format!("In use: {reason}. Delete it once that has stopped.")
}

/// What a surface says about a Delete folder request: the sentence, and
/// whether it is a warning. Built once, for both surfaces.
pub fn leftover_removal_report(
    path: &str,
    outcome: &LeftoverRemovalOutcome,
) -> crate::worktree_manager::RemovalReport {
    use crate::worktree_manager::RemovalReport;
    let warning = |message| RemovalReport {
        message,
        warning: true,
    };
    match outcome {
        LeftoverRemovalOutcome::NotManaged => warning(crate::status_text![
            "Nothing was deleted: ",
            n(path),
            " is not inside the folder dux keeps this project's worktrees in."
        ]),
        LeftoverRemovalOutcome::HoldsProjectCheckout => warning(crate::status_text![
            "Nothing was deleted: ",
            n(path),
            " holds this project's own repository."
        ]),
        LeftoverRemovalOutcome::NotLeftover => warning(crate::status_text![
            "Nothing was deleted: dux has no record of a left-over folder at ",
            n(path),
            "; refresh the worktree manager."
        ]),
        LeftoverRemovalOutcome::Unreadable { reason } => warning(crate::status_text![
            "Nothing was deleted: dux could not read ",
            n(path),
            format!(" ({reason}).")
        ]),
        LeftoverRemovalOutcome::Symlink => warning(crate::status_text![
            "Nothing was deleted: ",
            n(path),
            " is reached through a symbolic link, and dux deletes only a real folder it made."
        ]),
        LeftoverRemovalOutcome::GitUnknown { reason } => warning(crate::status_text![
            "Nothing was deleted: git could not say whether ",
            n(path),
            format!(" is or holds a worktree: {reason}")
        ]),
        LeftoverRemovalOutcome::Registered => warning(crate::status_text![
            "Nothing was deleted: git lists ",
            n(path),
            " as a worktree again; remove it as a worktree instead."
        ]),
        LeftoverRemovalOutcome::HoldsCheckout { path: inner } => warning(crate::status_text![
            "Nothing was deleted: ",
            n(path),
            " holds another checkout at ",
            n(inner.display()),
            "; move it out first."
        ]),
        LeftoverRemovalOutcome::Unlookable {
            path: inner,
            reason,
        } => warning(crate::status_text![
            "Nothing was deleted: dux could not look inside ",
            n(inner.display()),
            format!(" ({reason}), so it cannot tell whether another checkout is in "),
            n(path),
            "."
        ]),
        LeftoverRemovalOutcome::BeingRemoved => warning(crate::status_text![
            "Nothing was deleted: ",
            n(path),
            " is already being deleted."
        ]),
        LeftoverRemovalOutcome::Busy { reason } => warning(crate::status_text![
            "Nothing was deleted from ",
            n(path),
            format!(": {reason}. Try again once it has stopped.")
        ]),
        LeftoverRemovalOutcome::StillThere {
            leftovers,
            more,
            error,
        } => {
            let mut message = crate::status_text!["dux could not delete everything in ", n(path)];
            if !leftovers.is_empty() {
                message.push(" (");
                let names: Vec<&str> = leftovers.iter().map(String::as_str).collect();
                push_names(&mut message, &names);
                if *more > 0 {
                    message.push(format!(" and {more} more"));
                }
                message.push(")");
            }
            message.push(format!(
                ": {error}. Stop whatever is still writing there, then delete the folder again."
            ));
            warning(message)
        }
        LeftoverRemovalOutcome::Removed { branches } => branches_report(
            crate::status_text!["Deleted the left-over folder at ", n(path)],
            FolderOnDisk::Present,
            branches,
        ),
        LeftoverRemovalOutcome::FolderAlreadyGone { branches } => branches_report(
            crate::status_text!["The left-over folder at ", n(path), " was already gone"],
            FolderOnDisk::Gone,
            branches,
        ),
    }
}

/// `opening`, then what became of each branch. With the folder there (and now
/// deleted), the deleted branches finish the opening sentence; with it already
/// gone they get one of their own.
fn branches_report(
    mut message: crate::status_text::StatusText,
    folder: FolderOnDisk,
    branches: &[LeftoverBranchOutcome],
) -> crate::worktree_manager::RemovalReport {
    let named = |wanted: fn(&BranchFate) -> bool| -> Vec<&str> {
        branches
            .iter()
            .filter(|branch| wanted(&branch.fate))
            .map(|branch| branch.name.as_str())
            .collect()
    };
    let deleted = named(|fate| *fate == BranchFate::Deleted);
    let kept = named(|fate| *fate == BranchFate::Kept);
    let gone = named(|fate| *fate == BranchFate::AlreadyGone);
    let moved = named(|fate| *fate == BranchFate::Moved);
    let plural = |names: &[&str], one: &'static str, many: &'static str| {
        if names.len() == 1 { one } else { many }
    };
    let folder_deleted = folder == FolderOnDisk::Present;
    if deleted.is_empty() || !folder_deleted {
        message.push(".");
    }
    if !deleted.is_empty() {
        message.push(if folder_deleted {
            plural(&deleted, " and its branch ", " and its branches ")
        } else {
            plural(&deleted, " Deleted its branch ", " Deleted its branches ")
        });
        push_names(&mut message, &deleted);
        message.push(".");
    }
    if !kept.is_empty() {
        message.push(plural(&kept, " Kept its branch ", " Kept its branches "));
        push_names(&mut message, &kept);
        message.push(", as you chose.");
    }
    if !gone.is_empty() {
        message.push(plural(&gone, " Branch ", " Branches "));
        push_names(&mut message, &gone);
        message.push(plural(&gone, " was already gone.", " were already gone."));
    }
    if !moved.is_empty() {
        message.push(plural(&moved, " Branch ", " Branches "));
        push_names(&mut message, &moved);
        message.push(plural(
            &moved,
            " changed since the folder was left behind, so it was kept.",
            " changed since the folder was left behind, so they were kept.",
        ));
    }
    let mut warning = !moved.is_empty();
    for branch in branches {
        match &branch.fate {
            BranchFate::Refused { reason } => {
                warning = true;
                message.push(" ");
                message.push(git::branch_refusal_note(&branch.name, reason));
            }
            BranchFate::CheckFailed { reason } => {
                warning = true;
                message.push(crate::status_text![
                    " git could not check branch ",
                    n(branch.name),
                    format!(" ({reason}), so it was kept; try again later.")
                ]);
            }
            BranchFate::Unverifiable => {
                warning = true;
                message.push(crate::status_text![
                    " Branch ",
                    n(branch.name),
                    " was kept: dux has no record of the commit it was on when the folder was \
                     left behind, so it cannot tell whether it changed since. Delete it yourself \
                     with ",
                    n(git::branch_delete_command(&branch.name)),
                    " if you are sure."
                ]);
            }
            _ => {}
        }
    }
    crate::worktree_manager::RemovalReport { message, warning }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::test_scratch::ScratchDir;

    fn git(dir: &Path, args: &[&str]) {
        let out = crate::test_git::fixture_git()
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    struct Fixture {
        engine: Engine,
        _tmp: ScratchDir,
        repo: PathBuf,
        folder: PathBuf,
    }

    /// A project whose dux worktree `wt` on `feat` (born on `feat-old`) git
    /// let go of while something was writing into it: the folder is left,
    /// with `.astro/state` in it, and recorded as the agent delete records it.
    fn fixture() -> Fixture {
        let (mut engine, tmp) = test_engine();
        let repo = std::fs::canonicalize(tmp.path()).unwrap().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["commit", "--allow-empty", "-m", "init"]);
        git(&repo, &["branch", "feat-old"]);
        let project = sample_project("p1", repo.to_str().unwrap());
        let root = engine.paths.worktrees_root.join(&project.name);
        std::fs::create_dir_all(&root).unwrap();
        let folder = std::fs::canonicalize(&root).unwrap().join("wt");
        git(
            &repo,
            &["worktree", "add", "-b", "feat", folder.to_str().unwrap()],
        );
        git(
            &repo,
            &["worktree", "remove", "--force", folder.to_str().unwrap()],
        );
        std::fs::create_dir_all(folder.join(".astro")).unwrap();
        std::fs::write(folder.join(".astro/state"), "x").unwrap();
        engine
            .session_store
            .upsert_project(&crate::config::ProjectConfig {
                id: "p1".to_string(),
                path: repo.to_string_lossy().into_owned(),
                name: None,
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: std::collections::BTreeMap::new(),
            })
            .unwrap();
        engine.projects.push(project);
        let err = git::remove_worktree(&repo, &folder, "feat", Some("feat-old"))
            .expect_err("git no longer lists the folder");
        let err = record_if_leftover(
            &engine.paths.sessions_db_path,
            &repo,
            "p1",
            Some(BranchProvenance::CreatedByDux),
            err,
        );
        let leftover = err
            .downcast_ref::<git::LeftoverWorktreeFolder>()
            .expect("a left-over folder");
        assert!(leftover.in_manager, "recorded: {leftover}");
        Fixture {
            engine,
            _tmp: tmp,
            repo,
            folder,
        }
    }

    fn delete(fx: &Fixture, branches: &[&str]) -> LeftoverRemovalOutcome {
        match fx
            .engine
            .admit_leftover_removal(
                "p1",
                &fx.folder,
                branches.iter().map(|b| (*b).to_string()).collect(),
            )
            .expect("a known project")
        {
            LeftoverAdmission::Refused(outcome) => outcome,
            LeftoverAdmission::Admitted(ticket) => ticket.run().expect("the delete answers"),
        }
    }

    fn listed(fx: &Fixture) -> Vec<LeftoverFolder> {
        list_leftover_folders(
            &fx.engine.projects[0],
            &fx.engine.paths,
            &fx.engine.removal_coordination.ops,
            &[],
        )
    }

    fn report(fx: &Fixture, outcome: &LeftoverRemovalOutcome) -> String {
        leftover_removal_report(fx.folder.to_str().unwrap(), outcome)
            .message
            .to_string()
    }

    #[test]
    fn deleting_the_folder_deletes_only_the_ticked_branch_and_keeps_the_record_for_the_other() {
        let fx = fixture();
        assert_eq!(listed(&fx)[0].branches, ["feat", "feat-old"]);
        let outcome = delete(&fx, &["feat"]);
        assert!(!fx.folder.exists());
        assert!(!exists(&fx, "feat"));
        assert!(exists(&fx, "feat-old"));
        assert_eq!(
            report(&fx, &outcome),
            format!(
                "Deleted the left-over folder at {} and its branch feat. Kept its branch \
                 feat-old, as you chose.",
                fx.folder.display()
            )
        );
        // The branch kept by choice is still something to do: the row stays,
        // about it alone, until it is gone too.
        let rows = listed(&fx);
        assert!(!rows[0].folder_present);
        assert_eq!(rows[0].branches, ["feat-old"]);
        git(&fx.repo, &["branch", "-D", "feat-old"]);
        assert_eq!(listed(&fx), []);
    }

    /// Each branch is reported for what happened to it: one deleted, one
    /// that moved since the folder was left behind kept and said so.
    #[test]
    fn a_mixed_outcome_names_each_branch_for_what_happened_to_it() {
        let fx = fixture();
        git(&fx.repo, &["commit", "--allow-empty", "-m", "later"]);
        git(&fx.repo, &["branch", "-f", "feat-old", "HEAD"]);
        let outcome = delete(&fx, &["feat", "feat-old"]);
        assert!(exists(&fx, "feat-old"));
        let message = report(&fx, &outcome);
        assert_eq!(
            message,
            format!(
                "Deleted the left-over folder at {} and its branch feat. Branch feat-old \
                 changed since the folder was left behind, so it was kept.",
                fx.folder.display()
            )
        );
        assert!(
            leftover_removal_report(fx.folder.to_str().unwrap(), &outcome).warning,
            "a branch that was not deleted as asked is a warning"
        );
    }

    #[test]
    fn a_folder_that_will_not_go_is_reported_with_what_is_left_and_stays_listed() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            eprintln!("SKIPPED: root can empty a directory with no write permission");
            return;
        }
        let fx = fixture();
        let held = fx.folder.join(".astro");
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o555)).unwrap();
        let outcome = delete(&fx, &["feat"]);
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(&outcome, LeftoverRemovalOutcome::StillThere { leftovers, .. }
                if leftovers == &[".astro/state".to_string()]),
            "{outcome:?}"
        );
        assert!(exists(&fx, "feat"));
        assert_eq!(listed(&fx).len(), 1);
    }

    #[test]
    fn a_folder_outside_the_projects_worktrees_root_is_refused() {
        let fx = fixture();
        let outside = fx.repo.parent().unwrap().join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        let outcome = fx
            .engine
            .admit_leftover_removal("p1", &outside, Vec::new())
            .unwrap();
        assert!(matches!(
            outcome,
            LeftoverAdmission::Refused(LeftoverRemovalOutcome::NotManaged)
        ));
        assert!(outside.exists());
    }

    fn exists(fx: &Fixture, branch: &str) -> bool {
        git::resolve_branch_oid(&fx.repo, branch)
            .expect("git answers")
            .is_some()
    }

    fn fates(outcome: &LeftoverRemovalOutcome) -> Vec<(String, BranchFate)> {
        match outcome {
            LeftoverRemovalOutcome::Removed { branches }
            | LeftoverRemovalOutcome::FolderAlreadyGone { branches } => branches
                .iter()
                .map(|branch| (branch.name.clone(), branch.fate.clone()))
                .collect(),
            other => panic!("not a delete that went ahead: {other:?}"),
        }
    }

    /// A link at the path is not the folder that was left behind: the folder
    /// counts as gone, the link and its target are never deleted, and the
    /// ticked branch still goes.
    #[test]
    fn a_link_at_the_folders_path_counts_as_the_folder_gone_and_its_target_is_kept() {
        let fx = fixture();
        let target = fx.repo.parent().unwrap().join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep"), "x").unwrap();
        std::fs::rename(&fx.folder, fx.folder.with_extension("away")).unwrap();
        std::os::unix::fs::symlink(&target, &fx.folder).unwrap();
        let outcome = delete(&fx, &["feat"]);
        assert!(matches!(
            outcome,
            LeftoverRemovalOutcome::FolderAlreadyGone { .. }
        ));
        assert!(target.join("keep").exists());
        assert!(std::fs::symlink_metadata(&fx.folder).is_ok());
        assert!(!exists(&fx, "feat"));
    }

    /// The folder deleted and a new one made at the same path (which may
    /// reuse the inode): it is a different folder, never deleted, and the
    /// listing shows the record as a folder already removed with its branches.
    #[test]
    fn a_new_directory_at_the_path_is_never_deleted() {
        let fx = fixture();
        std::fs::remove_dir_all(&fx.folder).unwrap();
        std::fs::create_dir(&fx.folder).unwrap();
        std::fs::write(fx.folder.join("new"), "x").unwrap();
        let rows = listed(&fx);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].folder_present);
        let outcome = delete(&fx, &[]);
        assert!(matches!(
            outcome,
            LeftoverRemovalOutcome::FolderAlreadyGone { .. }
        ));
        assert!(fx.folder.join("new").exists());
        assert!(exists(&fx, "feat"));
    }

    /// A folder deleted by hand while its branches are kept stays listed, so
    /// the branches the message promised the manager would delete can still
    /// be deleted; the delete takes only the ticked ones and forgets it.
    #[test]
    fn a_folder_removed_by_hand_stays_listed_for_its_kept_branches() {
        let fx = fixture();
        std::fs::remove_dir_all(&fx.folder).unwrap();
        let rows = listed(&fx);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(!rows[0].folder_present);
        let outcome = delete(&fx, &["feat"]);
        assert_eq!(
            fates(&outcome),
            [
                ("feat".to_string(), BranchFate::Deleted),
                ("feat-old".to_string(), BranchFate::Kept),
            ]
        );
        assert!(!exists(&fx, "feat"));
        assert!(exists(&fx, "feat-old"));
        assert_eq!(
            report(&fx, &outcome),
            format!(
                "The left-over folder at {} was already gone. Deleted its branch feat. Kept \
                 its branch feat-old, as you chose.",
                fx.folder.display()
            )
        );
        // The branch kept by choice is still something to do: the row stays,
        // about it alone, until it is gone too.
        let rows = listed(&fx);
        assert!(!rows[0].folder_present);
        assert_eq!(rows[0].branches, ["feat-old"]);
        git(&fx.repo, &["branch", "-D", "feat-old"]);
        assert_eq!(listed(&fx), []);
    }

    #[test]
    fn a_folder_gone_with_none_of_its_branches_left_is_forgotten() {
        let fx = fixture();
        std::fs::remove_dir_all(&fx.folder).unwrap();
        git(&fx.repo, &["branch", "-D", "feat", "feat-old"]);
        assert_eq!(listed(&fx), []);
        let store =
            crate::storage::SessionStore::open_live(&fx.engine.paths.sessions_db_path).unwrap();
        assert_eq!(store.load_leftover_folders("p1").unwrap(), []);
    }

    /// A git that cannot be asked about a branch never reads as "already
    /// gone": each branch says it could not be checked, and the record stays
    /// for a later try.
    #[test]
    fn a_branch_git_cannot_check_is_kept_and_said_so() {
        let mut fx = fixture();
        std::fs::remove_dir_all(&fx.folder).unwrap();
        let not_a_repo = fx.repo.parent().unwrap().join("not-a-repo");
        std::fs::create_dir_all(&not_a_repo).unwrap();
        assert!(git::resolve_branch_oid(&not_a_repo, "feat").is_err());
        fx.engine.projects[0].path = not_a_repo.to_string_lossy().into_owned();
        let outcome = delete(&fx, &["feat"]);
        assert!(
            fates(&outcome)
                .iter()
                .all(|(_, fate)| matches!(fate, BranchFate::CheckFailed { .. })),
            "{outcome:?}"
        );
        assert!(report(&fx, &outcome).contains("could not check"));
        let store =
            crate::storage::SessionStore::open_live(&fx.engine.paths.sessions_db_path).unwrap();
        assert_eq!(store.load_leftover_folders("p1").unwrap().len(), 1);
    }

    /// A branch recorded with no commit (git could not be asked when the
    /// folder was left behind) is never deleted: compare-and-delete needs one.
    #[test]
    fn a_branch_recorded_without_a_commit_is_never_deleted() {
        let fx = fixture();
        let unverifiable = KeptBranch {
            name: "feat".to_string(),
            oid: None,
            provenance: None,
        };
        assert_eq!(
            git::delete_kept_branch(&fx.repo, &unverifiable).unwrap(),
            KeptBranchDeletion::Unverifiable
        );
        assert!(exists(&fx, "feat"));
    }

    #[test]
    fn a_folder_reached_through_a_swapped_ancestor_link_is_refused() {
        let fx = fixture();
        let root = fx.folder.parent().unwrap().to_path_buf();
        let real = root.with_extension("real");
        std::fs::rename(&root, &real).unwrap();
        std::os::unix::fs::symlink(&real, &root).unwrap();
        assert_eq!(delete(&fx, &["feat"]), LeftoverRemovalOutcome::Symlink);
        assert!(real.join("wt/.astro/state").exists());
        assert!(exists(&fx, "feat"));
    }

    #[test]
    /// git's own list is asked, not only the disk: the nested worktree's
    /// `.git` file is gone, so only the listing can see it.
    fn a_folder_holding_another_worktree_of_the_repository_is_refused() {
        let fx = fixture();
        let inner = fx.folder.join("inner");
        git(
            &fx.repo,
            &["worktree", "add", "-b", "inner", inner.to_str().unwrap()],
        );
        std::fs::remove_file(inner.join(".git")).unwrap();
        assert_eq!(
            delete(&fx, &[]),
            LeftoverRemovalOutcome::HoldsCheckout {
                path: inner.clone()
            }
        );
        assert!(inner.exists());
    }

    #[test]
    fn a_folder_with_a_subfolder_dux_cannot_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            eprintln!("SKIPPED: root reads every folder");
            return;
        }
        let fx = fixture();
        let closed = fx.folder.join("closed");
        std::fs::create_dir(&closed).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let outcome = delete(&fx, &[]);
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(&outcome, LeftoverRemovalOutcome::Unlookable { path, .. } if *path == closed),
            "{outcome:?}"
        );
        assert!(report(&fx, &outcome).contains("could not look inside"));
        assert!(fx.folder.exists());
    }

    /// A branch the delete could not remove keeps the record: there is still
    /// something to do with it, so the manager keeps listing it.
    #[test]
    fn a_kept_branch_a_delete_could_not_remove_keeps_the_record() {
        let fx = fixture();
        let elsewhere = fx.repo.parent().unwrap().join("elsewhere");
        git(
            &fx.repo,
            &["worktree", "add", elsewhere.to_str().unwrap(), "feat"],
        );
        let before = listed(&fx);
        assert_eq!(before[0].branches, ["feat-old"]);
        assert!(matches!(
            before[0].undeletable[0].why,
            BranchBlock::CheckedOut { .. }
        ));
        let outcome = delete(&fx, &["feat", "feat-old"]);
        assert!(!fx.folder.exists());
        assert_eq!(
            fates(&outcome)
                .into_iter()
                .map(|(name, fate)| (name, matches!(fate, BranchFate::Refused { .. })))
                .collect::<Vec<_>>(),
            [("feat".to_string(), true), ("feat-old".to_string(), false)]
        );
        let after = listed(&fx);
        assert_eq!(after.len(), 1, "{after:?}");
        assert!(!after[0].folder_present);
        assert_eq!(after[0].branches, Vec::<String>::new());
        assert_eq!(after[0].undeletable[0].name, "feat");
        assert!(
            !after[0].offers_delete(),
            "nothing a delete could do is left"
        );
        git(
            &fx.repo,
            &["worktree", "remove", "--force", elsewhere.to_str().unwrap()],
        );
        git(&fx.repo, &["branch", "-D", "feat"]);
        assert_eq!(listed(&fx), []);
    }

    #[test]
    fn a_kept_branch_that_moved_is_listed_as_kept_with_why() {
        let fx = fixture();
        git(&fx.repo, &["commit", "--allow-empty", "-m", "later"]);
        git(&fx.repo, &["branch", "-f", "feat-old", "HEAD"]);
        let rows = listed(&fx);
        assert_eq!(rows[0].branches, ["feat"]);
        assert_eq!(
            rows[0].undeletable,
            [UndeletableBranch {
                name: "feat-old".to_string(),
                why: BranchBlock::Moved,
            }]
        );
    }

    /// Recording a path again keeps the earlier record's branches that are
    /// still there, each with its own provenance, beside the new ones.
    #[test]
    fn recording_a_path_again_keeps_the_earlier_branches_still_there() {
        let fx = fixture();
        git(&fx.repo, &["branch", "-D", "feat-old"]);
        git(&fx.repo, &["branch", "again"]);
        let head = git::resolve_branch_oid(&fx.repo, "again").unwrap();
        let mut leftover = git::LeftoverWorktreeFolder {
            path: fx.folder.clone(),
            leftovers: Vec::new(),
            more: 0,
            git_error: String::new(),
            kept_branches: vec![KeptBranch {
                name: "again".to_string(),
                oid: head,
                provenance: None,
            }],
            gone_branches: Vec::new(),
            unchecked_branches: Vec::new(),
            in_manager: false,
        };
        record_leftover(
            &fx.engine.paths.sessions_db_path,
            &fx.repo,
            "p1",
            Some(BranchProvenance::AttachedExisting),
            &mut leftover,
        );
        assert!(leftover.in_manager);
        let store =
            crate::storage::SessionStore::open_live(&fx.engine.paths.sessions_db_path).unwrap();
        let record = store.load_leftover_folders("p1").unwrap().remove(0);
        assert_eq!(
            record
                .branches
                .iter()
                .map(|kept| (kept.name.as_str(), kept.provenance))
                .collect::<Vec<_>>(),
            [
                ("again", Some(BranchProvenance::AttachedExisting)),
                ("feat", Some(BranchProvenance::CreatedByDux)),
            ]
        );
    }

    #[test]
    fn a_folder_holding_another_repository_is_refused() {
        let fx = fixture();
        let clone = fx.folder.join("vendor/lib");
        std::fs::create_dir_all(&clone).unwrap();
        git(&clone, &["init"]);
        let outcome = delete(&fx, &[]);
        assert!(
            matches!(outcome, LeftoverRemovalOutcome::HoldsCheckout { .. }),
            "{outcome:?}"
        );
        assert!(report(&fx, &outcome).contains("vendor/lib"));
        assert!(clone.join(".git").exists());
    }

    #[test]
    fn a_folder_holding_the_projects_own_checkout_says_so() {
        let mut fx = fixture();
        let root = fx.engine.paths.worktrees_root.join("p2-name");
        let holder = root.join("holder");
        std::fs::create_dir_all(holder.join("repo")).unwrap();
        fx.engine
            .projects
            .push(sample_project("p2", holder.join("repo").to_str().unwrap()));
        let outcome = match fx
            .engine
            .admit_leftover_removal("p2", &holder, Vec::new())
            .unwrap()
        {
            LeftoverAdmission::Refused(outcome) => outcome,
            LeftoverAdmission::Admitted(_) => panic!("admitted"),
        };
        assert_eq!(outcome, LeftoverRemovalOutcome::HoldsProjectCheckout);
        let said = leftover_removal_report(holder.to_str().unwrap(), &outcome)
            .message
            .to_string();
        assert!(
            said.contains("holds this project's own repository"),
            "{said}"
        );
    }

    #[test]
    fn a_folder_a_terminal_is_open_in_is_refused() {
        let mut fx = fixture();
        let client = crate::pty::PtyClient::spawn_with_env(
            "cat",
            &[],
            &fx.folder.join(".astro"),
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn cat");
        fx.engine.companion_terminals.insert(
            "term-1".to_string(),
            crate::model::CompanionTerminal {
                owner: crate::model::TerminalOwner::Standalone,
                label: "shell".to_string(),
                foreground_cmd: None,
                client,
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        assert!(matches!(
            delete(&fx, &[]),
            LeftoverRemovalOutcome::Busy { .. }
        ));
        assert!(fx.folder.exists());
    }

    #[test]
    fn a_folder_a_standalone_agent_works_in_is_refused() {
        let mut fx = fixture();
        fx.engine
            .sessions
            .push(crate::engine::test_support::sample_standalone_session(
                "s3",
                fx.folder.join(".astro").to_str().unwrap(),
            ));
        assert!(matches!(
            delete(&fx, &[]),
            LeftoverRemovalOutcome::Busy { .. }
        ));
        assert!(fx.folder.exists());
    }

    #[test]
    fn a_folder_git_lists_as_a_worktree_again_is_refused() {
        let fx = fixture();
        std::fs::remove_dir_all(fx.folder.join(".astro")).unwrap();
        // git adds a worktree into an existing empty directory, the same one.
        git(
            &fx.repo,
            &[
                "worktree",
                "add",
                "-b",
                "again",
                fx.folder.to_str().unwrap(),
            ],
        );
        assert_eq!(delete(&fx, &[]), LeftoverRemovalOutcome::Registered);
        assert!(fx.folder.join(".git").exists());
    }

    #[test]
    fn a_git_that_cannot_answer_refuses_rather_than_deletes() {
        let mut fx = fixture();
        let not_a_repo = fx.repo.parent().unwrap().join("not-a-repo");
        std::fs::create_dir_all(&not_a_repo).unwrap();
        fx.engine.projects[0].path = not_a_repo.to_string_lossy().into_owned();
        assert!(matches!(
            delete(&fx, &[]),
            LeftoverRemovalOutcome::GitUnknown { .. }
        ));
        assert!(fx.folder.exists());
    }

    #[test]
    fn a_folder_an_agent_works_in_is_refused() {
        let mut fx = fixture();
        let mut session = sample_session("s2", "p1", "other");
        session.workspace.as_managed_mut().unwrap().worktree_path =
            fx.folder.to_string_lossy().into_owned();
        fx.engine.sessions.push(session);
        assert!(matches!(
            delete(&fx, &[]),
            LeftoverRemovalOutcome::Busy { .. }
        ));
        assert!(fx.folder.exists());
    }

    #[test]
    fn a_record_is_written_only_while_its_project_exists() {
        let fx = fixture();
        let store =
            crate::storage::SessionStore::open_live(&fx.engine.paths.sessions_db_path).unwrap();
        store.remove_project_records("p1").unwrap();
        assert_eq!(store.load_leftover_folders("p1").unwrap(), []);
        let identity = folder_identity(&fx.folder).unwrap().unwrap();
        let written = store
            .record_leftover_folder(&LeftoverFolderRecord {
                path: fx.folder.to_string_lossy().into_owned(),
                project_id: "p1".to_string(),
                branches: Vec::new(),
                provenance: None,
                identity,
            })
            .unwrap();
        assert!(!written);
    }
}
