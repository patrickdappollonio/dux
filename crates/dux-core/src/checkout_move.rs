//! The check in front of every pull and branch switch dux runs in a checkout.
//!
//! Moving a working tree to another commit makes git replace whatever HEAD
//! does not track and stands where the incoming commit needs room: an ignored
//! folder where a file goes is deleted with everything in it, an ignored or
//! untracked file where a file goes is overwritten, and one where a folder
//! goes is replaced. When that place is, or lies in, a standalone agent's
//! folder, another agent's worktree, a project's repository, or somewhere a
//! process dux started is working, that is the user's work gone. So before git
//! moves anything dux works out every such place from what is on disk, claims
//! it, and asks the occupancy rule both ways: what lives in it, and what it
//! lives in (short of the checkout itself, whose own agent and project contain
//! every path in it). A refusal moves nothing.
//!
//! Only what dux knows about is protected: git can still replace any other
//! ignored file or folder, and the docs say so.

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
    /// place and what lives there. `what` is the act, for that sentence
    /// ("pull", "switch to the branch"). Blocking: runs git, reads the
    /// session database and the process table, so call it on a worker, right
    /// before the move, and keep the clearance until the move is done.
    pub fn clear(&self, checkout: &Path, target: &str, what: &str) -> Result<MoveClearance> {
        crate::engine::destructive_guard::assert_off_engine_thread(
            "checking what a pull or branch switch would remove",
        );
        let changes = incoming_entries(checkout, target)?;
        let entries = &changes.entries;
        let tracked = tracked_at_head(checkout, &candidate_paths(checkout, entries))?;
        let mut locations =
            locations_git_would_replace(checkout, entries, &|path| tracked.contains(path));
        for deleted in &changes.deleted {
            let at = checkout.join(deleted);
            if crate::engine::is_symlink(&at) && !locations.iter().any(|known| known.path == at) {
                locations.push(Location {
                    path: at,
                    kind: LocationKind::LinkRemoved,
                });
            }
        }
        for folder in folders_left_empty(checkout, &changes) {
            if !locations.iter().any(|known| known.path == folder) {
                locations.push(Location {
                    path: folder,
                    kind: LocationKind::FolderEmptied,
                });
            }
        }
        let mut claims = Vec::with_capacity(locations.len());
        for location in &locations {
            let claim = self
                .ops
                .claim_for_destructive_as(
                    &location.path,
                    crate::worktree_ops::DESTRUCTIVE_CLAIM_WAIT,
                    CLAIMED_BY,
                )
                .map_err(|reason| refusal(checkout, location, what, &reason))?;
            claims.push(claim);
        }
        // A spawn anywhere in the checkout that began before the claims
        // registers its session in a moment; the looks below must see it.
        if !locations.is_empty()
            && !self
                .registry
                .wait_for_spawns(checkout, crate::process_sessions::SPAWN_WAIT)
        {
            return Err(anyhow!(
                "dux did not {what}: something dux is starting in {} has not finished starting; \
                 try again in a moment",
                crate::home_path::shorten_home(checkout)
            ));
        }
        for (location, claim) in locations.iter().zip(&claims) {
            // What lives in it (a folder or a link removed whole) ...
            if let Err((_, reason)) = self.check(&location.path).clear_or_say_why(&[claim]) {
                return Err(refusal(checkout, location, what, &reason));
            }
            // ... and what it lives in.
            if let Some(reason) = self.occupant_around(checkout, &location.path)? {
                return Err(refusal(checkout, location, what, &reason));
            }
        }
        Ok(MoveClearance { _claims: claims })
    }

    /// What `location` lies in: an agent's folder, a project's repository, a
    /// PTY's folder, a session dux started there that still runs, a process
    /// of dux's standing there, or an operation holding it. Only a folder
    /// short of the checkout itself counts: the checkout's own agent and
    /// project contain every path in it, and moving the checkout is theirs to
    /// ask for. `Err` when the session database cannot be read (fail closed).
    fn occupant_around(&self, checkout: &Path, location: &Path) -> Result<Option<String>> {
        let around = |dir: &Path| {
            crate::worktree_ops::folder_contains(dir, location)
                && !crate::worktree_ops::folder_contains(dir, checkout)
        };
        let (stored_agents, stored_projects) = crate::storage::SessionStore::open(&self.db_path)
            .and_then(|store| Ok((store.load_sessions()?, store.load_projects()?)))
            .map_err(|e| {
                anyhow!(
                    "dux could not read its list of agents and projects to confirm nothing lives \
                     there ({e:#})"
                )
            })?;
        if let Some(agent) = self
            .agents
            .iter()
            .chain(&stored_agents)
            .find(|agent| around(Path::new(agent.directory())))
        {
            return Ok(Some(
                crate::engine::Occupant::Agent {
                    id: agent.id.clone(),
                    label: agent.display_label(),
                    directory: agent.directory().to_string(),
                    standalone: agent.workspace.as_managed().is_none(),
                    exact: false,
                }
                .reason(),
            ));
        }
        let stored_projects: Vec<(String, String)> = stored_projects
            .into_iter()
            .map(|project| {
                (
                    project.name.clone().unwrap_or_else(|| project.path.clone()),
                    project.path,
                )
            })
            .collect();
        if let Some((name, path)) = self
            .projects
            .iter()
            .chain(&stored_projects)
            .find(|(_, path)| around(Path::new(path)))
        {
            return Ok(Some(
                crate::engine::Occupant::Project {
                    name: name.clone(),
                    path: path.clone(),
                }
                .reason(),
            ));
        }
        if let Some((_, _, what, _)) = self.ptys.iter().find(|(dir, ..)| around(dir)) {
            return Ok(Some((*what).to_string()));
        }
        if let Some((_, kinds)) = self.ops.holders_where(&|held| around(held)).first() {
            return Ok(Some(format!(
                "{} is running there",
                crate::worktree_ops::describe_holders(kinds)
            )));
        }
        let sessions = self
            .registry
            .sessions_matching(|started_in| around(started_in));
        if !sessions.is_empty() {
            let running = crate::process_sessions::members(
                &crate::process_sessions::read_process_table(),
                &sessions,
                &self.registry.survivors_of(&sessions),
                std::process::id(),
            );
            if !running.is_empty() {
                return Ok(Some(format!(
                    "something dux started there is still running ({})",
                    crate::process_sessions::describe(&running)
                )));
            }
        }
        Ok(self.registry.cwd_occupant_where(&around, &[]))
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

/// The error for a move refused because of `location`, with `reason` (a
/// phrase) saying what is in the way.
fn refusal(checkout: &Path, location: &Location, what: &str, reason: &str) -> anyhow::Error {
    let relative = location
        .path
        .strip_prefix(checkout)
        .unwrap_or(&location.path)
        .display();
    let change = match location.kind {
        LocationKind::FolderWhereFileGoes => format!(
            "tracks a file at {relative}, so git would delete the folder there with everything \
             in it"
        ),
        LocationKind::Overwritten => {
            format!("tracks {relative}, so git would overwrite the file or link that stands there")
        }
        LocationKind::FileWhereFolderGoes => format!(
            "needs a folder at {relative}, so git would replace the file or link that stands \
             there"
        ),
        LocationKind::LinkRemoved => {
            format!("deletes {relative}, so git would remove the link that stands there")
        }
        LocationKind::FolderEmptied => format!(
            "deletes everything in {relative}, so git would remove the folder itself once it is \
             empty"
        ),
    };
    anyhow!(
        "dux did not {what}: the commit it would check out {change}, and {reason}. Nothing in {} \
         changed",
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

/// What moving from HEAD to `target` changes, from plumbing with NUL-separated
/// paths: every path it creates or replaces, and every path it deletes. On an
/// unborn HEAD, every path of `target` and nothing deleted.
fn incoming_entries(checkout: &Path, target: &str) -> Result<IncomingChanges> {
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
        None => IncomingChanges {
            entries: parse_ls_tree(&output.stdout),
            deleted: Vec::new(),
        },
    })
}

/// What a move changes: the paths it writes, and the paths it deletes.
#[derive(Debug, Default, PartialEq, Eq)]
struct IncomingChanges {
    entries: Vec<IncomingEntry>,
    deleted: Vec<PathBuf>,
}

fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

/// `diff-tree -r -z --no-renames` output: a `:srcmode dstmode src dst status`
/// header, then the path, each NUL-terminated.
fn parse_raw_diff(out: &[u8]) -> IncomingChanges {
    let mut changes = IncomingChanges::default();
    let mut fields = out.split(|b| *b == 0);
    while let Some(header) = fields.next() {
        if header.is_empty() {
            continue;
        }
        let Some(path) = fields.next() else { break };
        let fields: Vec<&[u8]> = header
            .strip_prefix(b":")
            .unwrap_or_default()
            .split(|b| *b == b' ')
            .collect();
        let dst_mode = fields.get(1).copied().unwrap_or_default();
        if fields.get(4).is_some_and(|status| status.starts_with(b"D")) {
            changes.deleted.push(bytes_to_path(path));
            continue;
        }
        changes.entries.push(IncomingEntry {
            path: bytes_to_path(path),
            gitlink: dst_mode == b"160000",
        });
    }
    changes
}

/// Every folder git would remove because the move deletes everything in it:
/// after deleting its files, git removes each folder left empty. A folder
/// holding anything the move does not delete (an untracked or ignored file, a
/// path the incoming commit writes) stays. Only the outermost such folders
/// are answered; the ones inside them go with them. A folder whose contents
/// cannot be read is answered as removed, which only makes the check ask.
fn folders_left_empty(checkout: &Path, changes: &IncomingChanges) -> Vec<PathBuf> {
    let ignore_case = cfg!(target_os = "macos");
    // Names read from disk are compared with git's paths under the one
    // spelling rule: case folded where the filesystem folds it.
    let deleted: std::collections::HashSet<PathBuf> = changes
        .deleted
        .iter()
        .map(|path| crate::worktree_ops::case_key_with(path, ignore_case))
        .collect();
    let mut candidates: Vec<PathBuf> = Vec::new();
    for path in &changes.deleted {
        let mut at = path.parent();
        while let Some(folder) = at {
            if folder.as_os_str().is_empty() {
                break;
            }
            if !candidates.iter().any(|known| known == folder) {
                candidates.push(folder.to_path_buf());
            }
            at = folder.parent();
        }
    }
    let written_under = |folder: &Path| {
        changes
            .entries
            .iter()
            .any(|entry| crate::worktree_ops::spelled_under(&entry.path, folder))
    };
    let mut removed: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|folder| {
            !written_under(folder) && only_deleted_paths_in(checkout, folder, &deleted, ignore_case)
        })
        .collect();
    // Outermost only.
    let all = removed.clone();
    removed.retain(|folder| {
        !all.iter()
            .any(|other| other != folder && folder.starts_with(other))
    });
    removed
        .into_iter()
        .map(|folder| checkout.join(folder))
        .collect()
}

/// Whether everything on disk in `folder` (relative to `checkout`), at any
/// depth, is a path the move deletes. A real folder that is not there, or
/// cannot be read, counts as all deleted.
fn only_deleted_paths_in(
    checkout: &Path,
    folder: &Path,
    deleted: &std::collections::HashSet<PathBuf>,
    ignore_case: bool,
) -> bool {
    let is_deleted =
        |path: &Path| deleted.contains(&crate::worktree_ops::case_key_with(path, ignore_case));
    let at = checkout.join(folder);
    let Ok(meta) = std::fs::symlink_metadata(&at) else {
        return true;
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return is_deleted(folder);
    }
    let Ok(listing) = std::fs::read_dir(&at) else {
        return true;
    };
    for entry in listing {
        let Ok(entry) = entry else {
            return true;
        };
        let child = folder.join(entry.file_name());
        let is_dir = entry
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink());
        let gone = if is_dir {
            only_deleted_paths_in(checkout, &child, deleted, ignore_case)
        } else {
            is_deleted(&child)
        };
        if !gone {
            return false;
        }
    }
    true
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

/// What git would replace at one place on disk to write the incoming commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocationKind {
    /// A real folder standing where a file goes: removed whole.
    FolderWhereFileGoes,
    /// An untracked file, or any link, standing where a file goes: overwritten.
    Overwritten,
    /// An untracked file, or any link, standing where a folder goes: replaced.
    FileWhereFolderGoes,
    /// A folder whose every entry the move deletes: removed once empty.
    FolderEmptied,
    /// A link the move deletes, tracked or not.
    LinkRemoved,
}

/// One place git would overwrite or remove something HEAD does not track.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Location {
    path: PathBuf,
    kind: LocationKind,
}

/// Every path in `checkout` the walk in [`locations_git_would_replace`] may
/// ask HEAD about: each incoming path and each folder on the way to it.
fn candidate_paths(checkout: &Path, entries: &[IncomingEntry]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let mut at = PathBuf::new();
        for component in entry.path.components() {
            at.push(component);
            if std::fs::symlink_metadata(checkout.join(&at)).is_err() {
                break;
            }
            if !paths.contains(&at) {
                paths.push(at.clone());
            }
        }
    }
    paths
}

/// Which of `paths` (relative to `checkout`) HEAD tracks, as a file, a link or
/// a submodule. One `cat-file --batch-check` for all of them. A path holding a
/// newline cannot travel on its input and is answered "not tracked", which
/// only ever makes the check ask more.
fn tracked_at_head(
    checkout: &Path,
    paths: &[PathBuf],
) -> Result<std::collections::HashSet<PathBuf>> {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    let mut tracked = std::collections::HashSet::new();
    let Some(head) = commit_id(checkout, "HEAD")? else {
        return Ok(tracked);
    };
    let asked: Vec<&PathBuf> = paths
        .iter()
        .filter(|path| !path.as_os_str().as_bytes().contains(&b'\n'))
        .collect();
    if asked.is_empty() {
        return Ok(tracked);
    }
    let mut input = Vec::new();
    for path in &asked {
        input.extend_from_slice(head.as_bytes());
        input.push(b':');
        input.extend_from_slice(path.as_os_str().as_bytes());
        input.push(b'\n');
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["cat-file", "--batch-check=%(objecttype)"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("failed to run git cat-file in {}", checkout.display()))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output()?;
    let _ = writer.join();
    if !output.status.success() {
        return Err(anyhow!(
            "dux could not ask git what {} tracks",
            crate::home_path::shorten_home(checkout)
        ));
    }
    // One answer line per question, in order: a type, or `<object> missing`.
    for (path, answer) in asked.iter().zip(output.stdout.split(|b| *b == b'\n')) {
        if matches!(answer, b"blob" | b"commit") {
            tracked.insert((*path).clone());
        }
    }
    Ok(tracked)
}

/// Every place git would overwrite or remove something to write `entries`
/// into `checkout`: an untracked file, or any link, where a file goes; a real
/// folder where a file goes; and an untracked file, or any link, on the way to
/// one, where a folder goes. A FILE HEAD tracks is an ordinary update of its
/// content; a link never is, tracked or not. A folder on the way stays (the
/// incoming commit has a folder there too), and so does a real folder at a
/// submodule's own path.
fn locations_git_would_replace(
    checkout: &Path,
    entries: &[IncomingEntry],
    tracked: &dyn Fn(&Path) -> bool,
) -> Vec<Location> {
    let mut found: Vec<Location> = Vec::new();
    for entry in entries {
        let components: Vec<_> = entry.path.components().collect();
        let mut relative = PathBuf::new();
        for (index, component) in components.iter().enumerate() {
            relative.push(component);
            let at = checkout.join(&relative);
            let Ok(meta) = std::fs::symlink_metadata(&at) else {
                break;
            };
            let last = index + 1 == components.len();
            let kind = if meta.is_dir() && !meta.file_type().is_symlink() {
                if !last {
                    continue;
                }
                // A real folder at a submodule's path is kept (git checks
                // the submodule out into it).
                (!entry.gitlink).then_some(LocationKind::FolderWhereFileGoes)
            } else if tracked(&relative) && !meta.file_type().is_symlink() {
                // An ordinary update of a file's content. A link is never
                // that: removing or replacing it removes what it is, which
                // may be an agent's folder or a project's path recorded at it.
                None
            } else if !last {
                Some(LocationKind::FileWhereFolderGoes)
            } else {
                Some(LocationKind::Overwritten)
            };
            if let Some(kind) = kind
                && !found.iter().any(|known| known.path == at)
            {
                found.push(Location { path: at, kind });
            }
            break;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_from_disk_match_git_paths_under_the_case_rule() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Docs")).unwrap();
        std::fs::write(tmp.path().join("Docs/Guide.md"), "g").unwrap();
        let deleted: std::collections::HashSet<PathBuf> = [Path::new("docs/guide.md")]
            .iter()
            .map(|path| crate::worktree_ops::case_key_with(path, true))
            .collect();
        assert!(
            only_deleted_paths_in(tmp.path(), Path::new("Docs"), &deleted, true),
            "a case-folding filesystem: git's docs/guide.md is the Docs/Guide.md on disk"
        );
        let exact: std::collections::HashSet<PathBuf> =
            [PathBuf::from("docs/guide.md")].into_iter().collect();
        assert!(
            !only_deleted_paths_in(tmp.path(), Path::new("Docs"), &exact, false),
            "a case-sensitive one: they are different files"
        );
        assert_eq!(
            crate::worktree_ops::case_key_with(Path::new("A/B"), true),
            crate::worktree_ops::case_key_with(Path::new("a/b"), true)
        );
        assert_ne!(
            crate::worktree_ops::case_key_with(Path::new("A/B"), false),
            crate::worktree_ops::case_key_with(Path::new("a/b"), false)
        );
    }

    #[test]
    fn a_tracked_link_is_never_an_ordinary_update() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(tmp.path(), root.join("work")).unwrap();
        std::fs::write(root.join("plain"), "x").unwrap();
        let entries = vec![
            IncomingEntry {
                path: "work".into(),
                gitlink: false,
            },
            IncomingEntry {
                path: "plain".into(),
                gitlink: false,
            },
        ];
        let found = locations_git_would_replace(&root, &entries, &|_| true);
        assert_eq!(
            found
                .into_iter()
                .map(|location| (location.path, location.kind))
                .collect::<Vec<_>>(),
            vec![(root.join("work"), LocationKind::Overwritten)],
            "the tracked link is judged; the tracked file is an ordinary update"
        );
    }

    /// The places the walk finds, nothing tracked at HEAD.
    fn places(root: &Path, entries: &[IncomingEntry]) -> Vec<(PathBuf, LocationKind)> {
        locations_git_would_replace(root, entries, &|_| false)
            .into_iter()
            .map(|location| (location.path, location.kind))
            .collect()
    }

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
            places(root, &entries),
            vec![
                (root.join("scratch"), LocationKind::FolderWhereFileGoes),
                (root.join("plain"), LocationKind::Overwritten),
            ]
        );
        // A path HEAD tracks is an ordinary update.
        let tracked =
            locations_git_would_replace(root, &entries, &|path| path == Path::new("plain"));
        assert_eq!(tracked.len(), 1);
    }

    #[test]
    fn a_submodule_entry_removes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let entries = vec![IncomingEntry {
            path: "sub".into(),
            gitlink: true,
        }];
        assert!(places(tmp.path(), &entries).is_empty());
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
            places(&root, &entries),
            vec![(root.join("sub"), LocationKind::Overwritten)]
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
            places(&root, &entries),
            vec![(root.join("libs"), LocationKind::FileWhereFolderGoes)]
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
            places(&root, &entries),
            vec![(root.join("link"), LocationKind::FileWhereFolderGoes)]
        );
    }

    #[test]
    fn raw_diff_and_ls_tree_paths_are_read_byte_for_byte() {
        let raw = b":000000 100644 0000 1111 A\0odd name\nwith newline\0:100644 160000 1111 2222 T\0sub\0";
        assert_eq!(
            parse_raw_diff(raw).entries,
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
        let deleting = b":100644 000000 1111 0000 D\0docs/guide.md\0";
        let changes = parse_raw_diff(deleting);
        assert!(changes.entries.is_empty());
        assert_eq!(changes.deleted, vec![PathBuf::from("docs/guide.md")]);
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
