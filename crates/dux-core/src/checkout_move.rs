//! The check in front of every pull and branch switch dux runs in a checkout.
//!
//! Moving a working tree to another commit makes git delete whatever stands
//! where the incoming commit tracks a file: an ignored folder is expendable to
//! git, so it goes, with everything in it. When that folder is a standalone
//! agent's, a project's repository or another agent's worktree, that is the
//! user's work and its history gone. dux knows which folders it must never
//! remove, so before git moves anything it works out which folders the move
//! would remove and asks the one occupancy rule about each, under a claim, the
//! same way every destructive file operation does (see [`crate::destructive`]).
//! A refusal moves nothing.
//!
//! Only a folder dux knows about is protected: git can still remove any other
//! ignored folder, and the docs say so.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::process_sessions::AgentProcessRegistry;
use crate::worktree_ops::{DestructiveClaim, WorktreeOps};

/// What a claim made for a pull or switch is called while it holds a folder.
const CLAIMED_BY: &str = "a pull or branch switch";

/// The engine's facts a worker needs to ask whether moving a checkout's
/// working tree would remove a folder something lives in. Built on the engine
/// thread (`Engine::checkout_move_guard`), used off it.
#[derive(Clone, Default)]
pub struct CheckoutMoveGuard {
    registry: AgentProcessRegistry,
    db_path: PathBuf,
    ops: WorktreeOps,
    /// The agents and projects (by name and repository path) as the engine
    /// had them when the guard was built; the session database is asked again
    /// when a folder is cleared.
    agents: Vec<crate::model::AgentSession>,
    projects: Vec<(String, String)>,
    /// Every PTY dux ran when the guard was built, as the occupancy rule
    /// takes them.
    ptys: Vec<Pty>,
}

/// A PTY as the occupancy rule takes it: the folder it started in, its
/// process session, what to call it, and whether it is stopping.
pub(crate) type Pty = (
    PathBuf,
    Option<crate::process_sessions::ProcessSession>,
    &'static str,
    bool,
);

impl std::fmt::Debug for CheckoutMoveGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckoutMoveGuard")
            .field("db_path", &self.db_path)
            .field("agents", &self.agents.len())
            .field("projects", &self.projects)
            .finish()
    }
}

/// Holds the claims on every folder a cleared move would remove, so nothing
/// starts in one of them until git has moved the tree. Drop it after the move.
#[must_use = "the move must run while the clearance is held"]
pub struct MoveClearance {
    _claims: Vec<DestructiveClaim>,
}

impl CheckoutMoveGuard {
    pub(crate) fn new(
        registry: AgentProcessRegistry,
        db_path: PathBuf,
        ops: WorktreeOps,
        agents: Vec<crate::model::AgentSession>,
        projects: Vec<(String, String)>,
        ptys: Vec<Pty>,
    ) -> Self {
        Self {
            registry,
            db_path,
            ops,
            agents,
            projects,
            ptys,
        }
    }

    /// Clear moving `checkout`'s working tree from HEAD to the commit
    /// `target` (an object id), or refuse with the sentence that names the
    /// folder and what lives in it. `what` is the act, for that sentence
    /// ("pull", "switch to the branch"). Blocking: runs git, reads the
    /// session database and the process table, so call it on a worker, right
    /// before the move, and keep the clearance until the move is done.
    pub fn clear(&self, checkout: &Path, target: &str, what: &str) -> Result<MoveClearance> {
        crate::engine::destructive_guard::assert_off_engine_thread(
            "checking what a pull or branch switch would remove",
        );
        let entries = incoming_entries(checkout, target)?;
        let folders = folders_git_would_remove(checkout, &entries);
        let mut claims = Vec::with_capacity(folders.len());
        for folder in &folders {
            let claim = self
                .ops
                .claim_for_destructive_as(
                    folder,
                    crate::worktree_ops::DESTRUCTIVE_CLAIM_WAIT,
                    CLAIMED_BY,
                )
                .map_err(|reason| {
                    refusal(
                        checkout,
                        folder,
                        what,
                        &format!(
                            "dux did not let git delete {}: {reason}.",
                            crate::home_path::shorten_home(folder)
                        ),
                    )
                })?;
            claims.push(claim);
        }
        for (folder, claim) in folders.iter().zip(&claims) {
            self.check(folder)
                .clear(&[claim], "let git delete")
                .map_err(|refused| refusal(checkout, folder, what, &refused.0))?;
        }
        Ok(MoveClearance { _claims: claims })
    }

    /// The one occupancy rule for removing `folder` (the same question
    /// `Engine::destructive_check` asks), on the engine's facts as the guard
    /// holds them; the rest (agents and projects in the session database now,
    /// operations holding a path, sessions dux recorded and the working
    /// directories of what runs in them) is asked when it is cleared.
    fn check(&self, folder: &Path) -> crate::destructive::DestructiveCheck {
        let link = crate::engine::is_symlink(folder);
        let facts = crate::engine::OccupancyFacts {
            agents: &self.agents,
            projects: &self.projects,
            ops: &self.ops,
            // Removing a link leaves its target, and whoever works in it,
            // where they are.
            ptys: if link { Vec::new() } else { self.ptys.clone() },
        };
        let occupant = if link {
            crate::engine::link_occupant_in(folder, None, &facts)
        } else {
            crate::engine::occupant_in(
                folder,
                None,
                crate::engine::StoppingProcesses::Occupy,
                &facts,
            )
        }
        .map(|occupant| occupant.reason());
        let sessions = if link {
            self.registry.sessions_matching(|recorded| {
                crate::engine::recorded_at_or_through_link(folder, recorded)
            })
        } else {
            let mut sessions = self.registry.sessions_in(folder);
            sessions.extend(self.registry.standalone_sessions_in(folder));
            sessions
        };
        let known = self.registry.survivors_of(&sessions);
        crate::destructive::DestructiveCheck::new(
            folder,
            occupant,
            sessions,
            known,
            self.registry.clone(),
            self.db_path.clone(),
            self.ops.clone(),
        )
    }
}

/// The error for a move refused because of `folder`.
fn refusal(checkout: &Path, folder: &Path, what: &str, reason: &str) -> anyhow::Error {
    let relative = folder.strip_prefix(checkout).unwrap_or(folder);
    anyhow!(
        "{reason} The commit the {what} would check out tracks a file at {}, and git would \
         delete that folder, with everything in it, to make room, so dux did not {what}; nothing \
         in {} changed",
        relative.display(),
        crate::home_path::shorten_home(checkout)
    )
}

/// One path the move would write, and whether the incoming commit records a
/// submodule there (git keeps a real folder standing at that path, and
/// otherwise makes an empty one, replacing a link).
#[derive(Clone, Debug, PartialEq, Eq)]
struct IncomingEntry {
    path: PathBuf,
    gitlink: bool,
}

/// The object id `rev` names as a commit in `repo`, or `None` when it names
/// none. `rev` must not lead with a dash (an object id, `HEAD`, `FETCH_HEAD`
/// or a fully qualified ref).
pub(crate) fn commit_id(repo: &Path, rev: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .output()
        .with_context(|| format!("failed to run git rev-parse in {}", repo.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!id.is_empty()).then_some(id))
}

/// Every path moving from HEAD to `target` would create or replace (not the
/// ones it deletes), from plumbing with NUL-separated paths. On an unborn
/// HEAD, every path of `target`.
fn incoming_entries(checkout: &Path, target: &str) -> Result<Vec<IncomingEntry>> {
    let head = commit_id(checkout, "HEAD")?;
    let mut command = Command::new("git");
    command.arg("-C").arg(checkout);
    match &head {
        Some(head) => {
            command.args([
                "diff-tree",
                "-r",
                "-z",
                "--no-renames",
                // A submodule `.gitmodules` or the config marks `ignore = all`
                // still moves: list it whatever the repository says.
                "--ignore-submodules=none",
                "--no-commit-id",
                "--diff-filter=d",
                head,
                target,
            ]);
        }
        None => {
            command.args(["ls-tree", "-r", "-z", "--full-tree", target]);
        }
    }
    let output = command
        .output()
        .with_context(|| format!("failed to run git in {}", checkout.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "dux could not work out what the move would change in {}: {}",
            crate::home_path::shorten_home(checkout),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(match head {
        Some(_) => parse_raw_diff(&output.stdout),
        None => parse_ls_tree(&output.stdout),
    })
}

fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

/// `diff-tree -r -z --no-renames` output: a `:srcmode dstmode src dst status`
/// header, then the path, each NUL-terminated.
fn parse_raw_diff(out: &[u8]) -> Vec<IncomingEntry> {
    let mut entries = Vec::new();
    let mut fields = out.split(|b| *b == 0);
    while let Some(header) = fields.next() {
        if header.is_empty() {
            continue;
        }
        let Some(path) = fields.next() else { break };
        let dst_mode = header
            .strip_prefix(b":")
            .and_then(|rest| rest.split(|b| *b == b' ').nth(1))
            .unwrap_or_default();
        entries.push(IncomingEntry {
            path: bytes_to_path(path),
            gitlink: dst_mode == b"160000",
        });
    }
    entries
}

/// `ls-tree -r -z` output: `mode type id<TAB>path`, NUL-terminated.
fn parse_ls_tree(out: &[u8]) -> Vec<IncomingEntry> {
    out.split(|b| *b == 0)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let tab = record.iter().position(|b| *b == b'\t')?;
            let (meta, path) = record.split_at(tab);
            Some(IncomingEntry {
                path: bytes_to_path(&path[1..]),
                gitlink: meta.starts_with(b"160000"),
            })
        })
        .collect()
}

/// Every folder git would have to remove to write `entries` into
/// `checkout`: a real folder standing where a file (or link) goes, and a
/// symbolic link standing at any entry's path or anywhere on the way to one,
/// a submodule's included (git replaces it rather than write through it). A
/// folder on the way stays, since the incoming commit has a folder there too,
/// and so does a real folder at a submodule's own path.
fn folders_git_would_remove(checkout: &Path, entries: &[IncomingEntry]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let components: Vec<_> = entry.path.components().collect();
        let mut at = checkout.to_path_buf();
        for (index, component) in components.iter().enumerate() {
            at.push(component);
            let Ok(meta) = std::fs::symlink_metadata(&at) else {
                break;
            };
            let last = index + 1 == components.len();
            // A real folder at a submodule's path is kept (git checks the
            // submodule out into it); a link there is replaced by one.
            if meta.file_type().is_symlink() || (last && meta.is_dir() && !entry.gitlink) {
                if !found.contains(&at) {
                    found.push(at.clone());
                }
                break;
            }
            if !meta.is_dir() {
                break;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_where_a_file_goes_is_removed_and_one_on_the_way_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("scratch/inner")).unwrap();
        std::fs::create_dir_all(root.join("vendor/lib")).unwrap();
        std::fs::write(root.join("plain"), "file").unwrap();
        let entries = vec![
            IncomingEntry {
                path: "scratch".into(),
                gitlink: false,
            },
            IncomingEntry {
                path: "vendor/README".into(),
                gitlink: false,
            },
            IncomingEntry {
                path: "plain".into(),
                gitlink: false,
            },
            IncomingEntry {
                path: "new/dir/file".into(),
                gitlink: false,
            },
        ];
        assert_eq!(
            folders_git_would_remove(root, &entries),
            vec![root.join("scratch")]
        );
    }

    #[test]
    fn a_submodule_entry_removes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let entries = vec![IncomingEntry {
            path: "sub".into(),
            gitlink: true,
        }];
        assert!(folders_git_would_remove(tmp.path(), &entries).is_empty());
    }

    #[test]
    fn a_link_at_a_submodule_path_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("sub")).unwrap();
        let entries = vec![IncomingEntry {
            path: "sub".into(),
            gitlink: true,
        }];
        assert_eq!(
            folders_git_would_remove(&root, &entries),
            vec![root.join("sub")]
        );
    }

    #[test]
    fn a_link_on_the_way_to_a_submodule_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("sub")).unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("libs")).unwrap();
        let entries = vec![IncomingEntry {
            path: "libs/sub".into(),
            gitlink: true,
        }];
        assert_eq!(
            folders_git_would_remove(&root, &entries),
            vec![root.join("libs")]
        );
    }

    #[test]
    fn a_link_on_the_way_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("link")).unwrap();
        let entries = vec![IncomingEntry {
            path: "link/file".into(),
            gitlink: false,
        }];
        assert_eq!(
            folders_git_would_remove(&root, &entries),
            vec![root.join("link")]
        );
    }

    #[test]
    fn raw_diff_and_ls_tree_paths_are_read_byte_for_byte() {
        let raw = b":000000 100644 0000 1111 A\0odd name\nwith newline\0:100644 160000 1111 2222 T\0sub\0";
        assert_eq!(
            parse_raw_diff(raw),
            vec![
                IncomingEntry {
                    path: "odd name\nwith newline".into(),
                    gitlink: false
                },
                IncomingEntry {
                    path: "sub".into(),
                    gitlink: true
                },
            ]
        );
        let tree = b"100644 blob 1111\tscratch\x00160000 commit 2222\tsub\x00";
        assert_eq!(
            parse_ls_tree(tree),
            vec![
                IncomingEntry {
                    path: "scratch".into(),
                    gitlink: false
                },
                IncomingEntry {
                    path: "sub".into(),
                    gitlink: true
                },
            ]
        );
    }
}
