//! The one gate in front of every destructive file operation dux performs on
//! a folder: deleting an untracked folder or a nested repository from the
//! changes pane, and deleting or moving an entry from the editor.
//!
//! Each of those operations takes a [`Cleared`], and the only way to make one
//! is [`DestructiveCheck::clear`]: the engine builds the check from the one
//! occupancy question, and clearing it looks at what is running there. A
//! clearance names the exact paths it was granted for, and an operation on any
//! other path refuses, so an operation that skipped the check cannot be
//! written at all.

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
    ) -> Self {
        Self {
            targets: vec![CheckedTarget {
                path: path.to_path_buf(),
                occupant,
                sessions,
                known,
            }],
            registry,
        }
    }

    /// One check covering the paths of both (both ends of a move).
    pub fn and(mut self, other: DestructiveCheck) -> Self {
        self.targets.extend(other.targets);
        self
    }

    /// Clear the operation, or refuse it with the sentence that names what
    /// is in the way: an agent or project living there, an operation holding
    /// a path there, something dux started there still running, or a process
    /// dux started standing in it as its current folder. `what` is the verb
    /// ("delete", "move"). Blocking: reads the process table, so call it off
    /// the engine thread, right before the operation.
    pub fn clear(&self, what: &str) -> Result<Cleared, Refused> {
        for target in &self.targets {
            let reason = target
                .occupant
                .clone()
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
                .or_else(|| self.registry.cwd_occupant(&target.path, &[]));
            if let Some(reason) = reason {
                return Err(Refused(format!(
                    "dux did not {what} {}: {reason}.",
                    crate::home_path::shorten_home(&target.path)
                )));
            }
        }
        Ok(Cleared {
            keys: self
                .targets
                .iter()
                .map(|target| crate::worktree_ops::path_key(&target.path))
                .collect(),
            #[cfg(test)]
            any: false,
        })
    }
}

/// Proof that the paths it names were cleared for a destructive operation a
/// moment ago. Only [`DestructiveCheck::clear`] makes one.
#[derive(Debug)]
pub struct Cleared {
    keys: Vec<PathBuf>,
    #[cfg(test)]
    any: bool,
}

impl Cleared {
    /// Refuse unless `path` is one of the paths this clearance was granted
    /// for: a clearance for one folder never licenses another.
    pub fn require(&self, path: &Path) -> anyhow::Result<()> {
        #[cfg(test)]
        if self.any {
            return Ok(());
        }
        let key = crate::worktree_ops::path_key(path);
        if self.keys.contains(&key) {
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
    pub(crate) fn any_for_tests() -> Self {
        Self {
            keys: Vec::new(),
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

    #[test]
    fn a_clearance_covers_only_the_paths_it_was_granted_for() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let check = DestructiveCheck::new(
            &a,
            None,
            Vec::new(),
            Vec::new(),
            AgentProcessRegistry::default(),
        );
        let cleared = check.clear("delete").unwrap();
        cleared.require(&a).unwrap();
        assert!(cleared.require(&b).is_err());
        assert!(cleared.require(&a.join("inside")).is_err());
    }

    #[test]
    fn an_occupant_refuses_with_its_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let check = DestructiveCheck::new(
            tmp.path(),
            Some("project \"p\" has its repository at it".to_string()),
            Vec::new(),
            Vec::new(),
            AgentProcessRegistry::default(),
        );
        let refused = check.clear("move").unwrap_err();
        assert!(refused.0.starts_with("dux did not move "), "{refused}");
        assert!(refused.0.contains("project \"p\""), "{refused}");
    }
}
