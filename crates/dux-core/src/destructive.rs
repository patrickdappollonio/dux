//! The one gate in front of every destructive file operation dux performs on
//! a folder: deleting an untracked folder or a nested repository from the
//! changes pane, and deleting or moving an entry from the editor.
//!
//! The protocol has one order, and the types allow no other:
//!
//! 1. claim every target ([`crate::worktree_ops::WorktreeOps::claim_for_destructive`]),
//!    so nothing new can start in it from then on;
//! 2. ask the engine the one occupancy question (`Engine::destructive_check`);
//! 3. right before the operation, [`DestructiveCheck::clear`] it, handing in the
//!    claims: it asks the whole occupancy rule again (agents and projects as the
//!    session database has them now, and what is running or standing there) and
//!    gives a [`Cleared`] that borrows the claims, so it cannot outlive them;
//! 4. the operation takes that [`Cleared`], which names the exact paths it was
//!    granted for, and refuses any other.
//!
//! An operation that skipped the check, or a check that skipped the claim,
//! cannot be written at all.

use std::path::{Path, PathBuf};

use crate::process_sessions::{AgentProcessRegistry, ProcessIdentity, ProcessSession};

/// What stands in the way of deleting or moving one path and everything
/// under it, as the engine saw it, plus what still has to be looked at off
/// the engine thread.
#[derive(Clone, Debug)]
struct CheckedTarget {
    path: PathBuf,
    occupant: Option<String>,
    sessions: Vec<ProcessSession>,
    known: Vec<ProcessIdentity>,
}

/// Built on the engine thread by `Engine::destructive_check`; cleared off it.
#[derive(Clone)]
pub struct DestructiveCheck {
    targets: Vec<CheckedTarget>,
    registry: AgentProcessRegistry,
    /// The session database and the path registry, for asking the occupancy
    /// rule again at the moment of clearing.
    db_path: PathBuf,
    ops: crate::worktree_ops::WorktreeOps,
}

impl std::fmt::Debug for DestructiveCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DestructiveCheck")
            .field("targets", &self.targets)
            .finish()
    }
}

impl DestructiveCheck {
    pub(crate) fn new(
        path: &Path,
        occupant: Option<String>,
        sessions: Vec<ProcessSession>,
        known: Vec<ProcessIdentity>,
        registry: AgentProcessRegistry,
        db_path: PathBuf,
        ops: crate::worktree_ops::WorktreeOps,
    ) -> Self {
        Self {
            targets: vec![CheckedTarget {
                path: path.to_path_buf(),
                occupant,
                sessions,
                known,
            }],
            registry,
            db_path,
            ops,
        }
    }

    /// One check covering the paths of both (both ends of a move).
    pub fn and(mut self, other: DestructiveCheck) -> Self {
        self.targets.extend(other.targets);
        self
    }

    /// The refusal the engine's own state already gives (an agent or project
    /// living there, an operation holding a path there), with no blocking
    /// read: a caller on the engine thread can refuse at once instead of
    /// starting a worker that would only refuse later. `None` says only that
    /// [`Self::clear`] has more to look at.
    pub fn refused_now(&self, what: &str) -> Option<Refused> {
        self.targets.iter().find_map(|target| {
            target.occupant.as_ref().map(|reason| {
                Refused(format!(
                    "dux did not {what} {}: {reason}.",
                    crate::home_path::shorten_home(&target.path)
                ))
            })
        })
    }

    /// Clear the operation, or refuse it with the sentence that names what
    /// is in the way: an agent or project living there, an operation holding
    /// a path there, something dux started there still running, or a process
    /// dux started standing in it as its current folder. `what` is the verb
    /// ("delete", "move"). Every target must be held by one of `claims`; the
    /// clearance borrows them. Blocking: reads the session database and the
    /// process table, so call it off the engine thread, right before the
    /// operation.
    pub fn clear<'a>(
        &self,
        claims: &[&'a crate::worktree_ops::DestructiveClaim],
        what: &str,
    ) -> Result<Cleared<'a>, Refused> {
        self.clear_or_say_why(claims).map_err(|(path, reason)| {
            Refused(format!(
                "dux did not {what} {}: {reason}.",
                crate::home_path::shorten_home(&path)
            ))
        })
    }

    /// [`Self::clear`], answering a refusal with the path and the bare reason
    /// (a phrase), for a caller that words the sentence itself.
    pub(crate) fn clear_or_say_why<'a>(
        &self,
        claims: &[&'a crate::worktree_ops::DestructiveClaim],
    ) -> Result<Cleared<'a>, (PathBuf, String)> {
        crate::engine::destructive_guard::assert_off_engine_thread(
            "clearing a destructive operation",
        );
        for target in &self.targets {
            let refused = |reason: String| (target.path.clone(), reason);
            if !claims.iter().any(|claim| claim.covers(&target.path)) {
                return Err(refused(
                    "it was not claimed first, so something could still start in it".to_string(),
                ));
            }
            // A spawn that began before the claim registers its session in a
            // moment; the look below must see it.
            if !self
                .registry
                .wait_for_spawns(&target.path, crate::process_sessions::SPAWN_WAIT)
            {
                return Err(refused(
                    "something dux is starting in it has not finished starting".to_string(),
                ));
            }
            let reason = target
                .occupant
                .clone()
                .or_else(|| {
                    // The whole rule again, now: an agent created or a
                    // project added since the engine was asked is in the
                    // database before it is anywhere else.
                    match crate::engine::stored_occupant(
                        &self.db_path,
                        &self.ops,
                        &target.path,
                        None,
                    ) {
                        Ok(occupant) => occupant.map(|occupant| occupant.reason()),
                        Err(message) => Some(message),
                    }
                })
                .or_else(|| {
                    if target.sessions.is_empty() {
                        return None;
                    }
                    let running = crate::process_sessions::members(
                        &crate::process_sessions::read_process_table(),
                        &target.sessions,
                        &target.known,
                        std::process::id(),
                    );
                    (!running.is_empty()).then(|| {
                        format!(
                            "something dux started there is still running ({})",
                            crate::process_sessions::describe(&running)
                        )
                    })
                })
                .or_else(|| {
                    // A link's removal leaves its target, and whoever works
                    // in it, where they are.
                    if crate::engine::is_symlink(&target.path) {
                        return None;
                    }
                    self.registry.cwd_occupant(&target.path, &[])
                });
            if let Some(reason) = reason {
                return Err(refused(reason));
            }
        }
        Ok(Cleared {
            keys: self
                .targets
                .iter()
                .map(|target| crate::worktree_ops::lexical_key(&target.path))
                .collect(),
            _claims: std::marker::PhantomData,
            #[cfg(test)]
            any: false,
        })
    }
}

/// Proof that the paths it names were cleared for a destructive operation a
/// moment ago, while claimed. Only [`DestructiveCheck::clear`] makes one, and
/// it cannot outlive the claims it was made under.
#[derive(Debug)]
pub struct Cleared<'a> {
    keys: Vec<PathBuf>,
    _claims: std::marker::PhantomData<&'a crate::worktree_ops::DestructiveClaim>,
    #[cfg(test)]
    any: bool,
}

impl Cleared<'_> {
    /// Refuse unless `path` is one of the paths this clearance was granted
    /// for: a clearance for one folder never licenses another.
    pub fn require(&self, path: &Path) -> anyhow::Result<()> {
        #[cfg(test)]
        if self.any {
            return Ok(());
        }
        let key = crate::worktree_ops::lexical_key(path);
        if self
            .keys
            .iter()
            .any(|cleared| crate::worktree_ops::spelled_same(cleared, &key))
        {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "dux did not touch {}: it was not the folder that was checked for anything \
                 living in it",
                crate::home_path::shorten_home(path)
            ))
        }
    }

    /// A clearance for any path, for unit tests of the file operations
    /// themselves (which have no engine to ask).
    #[cfg(test)]
    pub(crate) fn any_for_tests() -> Cleared<'static> {
        Cleared {
            keys: Vec::new(),
            _claims: std::marker::PhantomData,
            any: true,
        }
    }
}

/// A destructive operation refused because something lives where it would
/// delete or move. Surfaces answer it as a conflict, with this sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(path: &Path, occupant: Option<String>, db: &Path) -> DestructiveCheck {
        DestructiveCheck::new(
            path,
            occupant,
            Vec::new(),
            Vec::new(),
            AgentProcessRegistry::default(),
            db.to_path_buf(),
            crate::worktree_ops::WorktreeOps::new(),
        )
    }

    #[test]
    fn a_clearance_covers_only_the_paths_it_was_granted_for() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let ops = crate::worktree_ops::WorktreeOps::new();
        let claim = ops.claim_for_destructive(&a).unwrap();
        let cleared = check(&a, None, &db).clear(&[&claim], "delete").unwrap();
        cleared.require(&a).unwrap();
        assert!(cleared.require(&b).is_err());
        assert!(cleared.require(&a.join("inside")).is_err());
    }

    #[test]
    fn a_check_without_a_claim_on_its_target_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let ops = crate::worktree_ops::WorktreeOps::new();
        let claim_on_b = ops.claim_for_destructive(&b).unwrap();
        let refused = check(&a, None, &db)
            .clear(&[&claim_on_b], "delete")
            .unwrap_err();
        assert!(refused.0.contains("not claimed first"), "{refused}");
    }

    #[test]
    fn an_occupant_refuses_with_its_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        let ops = crate::worktree_ops::WorktreeOps::new();
        let claim = ops.claim_for_destructive(tmp.path()).unwrap();
        let refused = check(
            tmp.path(),
            Some("project \"p\" has its repository at it".to_string()),
            &db,
        )
        .clear(&[&claim], "move")
        .unwrap_err();
        assert!(refused.0.starts_with("dux did not move "), "{refused}");
        assert!(refused.0.contains("project \"p\""), "{refused}");
    }

    /// A project written to the database after the engine was asked is
    /// found when clearing: the whole rule is asked again.
    #[test]
    fn a_project_added_after_the_check_refuses_the_clearance() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        let target = tmp.path().join("vendor");
        std::fs::create_dir_all(&target).unwrap();
        let ops = crate::worktree_ops::WorktreeOps::new();
        let claim = ops.claim_for_destructive(&target).unwrap();
        let pending = check(&target, None, &db);
        crate::storage::SessionStore::open(&db)
            .unwrap()
            .upsert_project(&crate::config::ProjectConfig {
                id: "p".to_string(),
                path: target.to_string_lossy().into_owned(),
                name: Some("vendor".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        let refused = pending.clear(&[&claim], "delete").unwrap_err();
        assert!(refused.0.contains("project \"vendor\""), "{refused}");
    }
}
