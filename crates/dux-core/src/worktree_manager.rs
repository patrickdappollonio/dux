//! The worktree manager's shared semantics: which of a project's git worktrees
//! a user may remove by hand, and what removing one does to its branch.
//!
//! This is the MANUAL OVERRIDE for deleting a branch. Deleting an agent honors
//! [`crate::model::BranchProvenance`] and keeps a branch dux did not create; the
//! worktree manager honors the checkbox the user ticked, whatever the branch's
//! origin, because the user is pointing at one specific worktree and saying so.
//! Both surfaces (the web's Worktrees dialog and the TUI's `manage-worktrees`
//! palette command) drive the rules below, so the two cannot answer differently.
//!
//! The rules, in one place:
//!
//! * A worktree is MANAGEABLE when it lives under dux's worktrees root for that
//!   project and is not the project checkout itself. An external worktree and
//!   the source checkout are not the manager's to touch.
//! * A manageable worktree is REMOVABLE when no agent holds it. An attached one
//!   is still listed (silently hiding it would leave the user hunting for a
//!   worktree they can see on disk), and refused with "delete the agent
//!   instead": removing it from under a live agent leaves a broken session.
//! * Removal is forced (`git worktree remove --force`); dux has no trash.
//! * The branch is deleted only when the caller asked AND the worktree is on
//!   one. A detached worktree has no branch to delete, so nothing is attempted
//!   and nothing may be claimed about one.
//!
//! Deciding is separated from doing ([`resolve_removal`] and
//! [`branch_to_delete`] are pure over a classification) so the rules are
//! testable without a git repository, and [`remove_managed_worktree`] does the
//! classification and the removal in ONE hop so the decision is always made
//! against a fresh listing rather than whatever a client last saw.

use std::path::{Path, PathBuf};

use crate::config::DuxPaths;
use crate::git;
use crate::model::{AgentSession, Project};
use crate::worker::ProjectWorktreeEntry;

/// One row of the worktree manager: a managed worktree of a project, and
/// everything both surfaces need to render and decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedWorktree {
    /// The canonical path git knows the worktree by.
    pub path: PathBuf,
    /// The row LABEL: the branch when there is one, else a "detached <sha>"
    /// stand-in. Good for display and useless for deciding anything.
    pub label: String,
    /// The real branch, `None` for a detached worktree. Separate from `label`
    /// because "is there a branch here to delete?" cannot be answered from a
    /// label that invents one.
    pub branch: Option<String>,
    /// Whether the worktree holds uncommitted work (staged, unstaged, or
    /// untracked). Both surfaces say so specifically in their confirmation,
    /// because removal is forced and there is no trash.
    pub dirty: bool,
    /// The agent holding this worktree, when one does. `Some` means the row is
    /// listed but not removable.
    pub attached_session_id: Option<String>,
}

impl ManagedWorktree {
    /// Whether the manager may remove this worktree. An agent holding it is the
    /// one reason it may not.
    pub fn is_removable(&self) -> bool {
        self.attached_session_id.is_none()
    }
}

/// Project a full worktree classification down to the manager's rows.
///
/// Pure, and the one place the "managed, and not the project checkout" filter
/// lives. Dirtiness is not knowable from a classification, so it comes back
/// `false` here and is filled in by [`list_manageable_worktrees`], which is
/// allowed to shell to git.
pub fn manageable_worktrees(entries: Vec<ProjectWorktreeEntry>) -> Vec<ManagedWorktree> {
    entries
        .into_iter()
        .filter(|entry| entry.is_managed_by_dux && !entry.is_project_checkout)
        .map(|entry| ManagedWorktree {
            path: entry.path,
            label: entry.branch_name,
            branch: entry.branch,
            dirty: false,
            attached_session_id: entry.existing_session_id,
        })
        .collect()
}

/// List a project's manageable worktrees, dirtiness included.
///
/// Shells to git twice over (one listing plus one `git status` per managed
/// worktree), so every caller must run it off the UI thread and off the async
/// reactor. A dirtiness check that fails (the directory vanished under us, a
/// git lock) degrades to "clean" rather than failing the whole listing: the
/// manager is still useful without the warning, and every confirmation says the
/// removal is forced anyway.
pub fn list_manageable_worktrees(
    project: &Project,
    paths: &DuxPaths,
    sessions: &[AgentSession],
) -> Result<Vec<ManagedWorktree>, String> {
    let worktrees = git::list_worktrees(Path::new(&project.path)).map_err(|e| format!("{e:#}"))?;
    let classified =
        crate::project_browser::classify_project_worktrees(project, paths, sessions, worktrees);
    Ok(manageable_worktrees(classified)
        .into_iter()
        .map(|mut entry| {
            entry.dirty = git::worktree_is_dirty(&entry.path).unwrap_or(false);
            entry
        })
        .collect())
}

/// What a removal request resolves to. Three answers, decided in one place, so
/// the web's status codes and the TUI's messages cannot disagree about which
/// case they are in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalResolution {
    /// Not a manageable worktree of this project. dux will not remove a
    /// directory it was not asked about.
    NotManaged,
    /// An agent holds it. Deleting the agent is the supported route.
    Attached,
    /// Removable; carries the canonical path git knows it by and the branch it
    /// is on (`None` when detached, which is nothing to delete).
    Removable {
        path: PathBuf,
        branch: Option<String>,
    },
}

/// Decide what a removal request resolves to, given a classification and the
/// path the caller asked about. Pure.
///
/// The comparison is canonical: the path a client echoes back came from this
/// module's own listing and is already canonical, but a symlinked temp root or
/// a hand-written request need not be.
pub fn resolve_removal(entries: Vec<ProjectWorktreeEntry>, requested: &Path) -> RemovalResolution {
    let wanted = std::fs::canonicalize(requested).unwrap_or_else(|_| requested.to_path_buf());
    match manageable_worktrees(entries)
        .into_iter()
        .find(|entry| entry.path == wanted)
    {
        None => RemovalResolution::NotManaged,
        Some(entry) if !entry.is_removable() => RemovalResolution::Attached,
        Some(entry) => RemovalResolution::Removable {
            path: entry.path,
            branch: entry.branch,
        },
    }
}

/// Which branch, if any, a removal should delete. Pure, and the whole rule:
/// the caller's request AND a branch to act on. A detached worktree sends
/// nothing regardless of what was asked.
pub fn branch_to_delete(delete_branch: bool, branch: Option<&str>) -> Option<&str> {
    match (delete_branch, branch) {
        (true, Some(branch)) => Some(branch),
        _ => None,
    }
}

/// What happened to the branch of a removed worktree, when one was targeted at
/// all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchOutcome {
    pub name: String,
    pub deletion: git::BranchDeletion,
}

/// The result of a removal request: the resolution, and (on the removable path)
/// what happened to the branch. `branch: None` means no branch deletion was
/// attempted, so no caller may claim one either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalOutcome {
    NotManaged,
    Attached,
    Removed {
        path: PathBuf,
        branch: Option<BranchOutcome>,
    },
}

/// Classify and remove in one hop: the entry point both surfaces call.
///
/// Shells to git, so run it off the UI thread and off the async reactor.
pub fn remove_managed_worktree(
    project: &Project,
    paths: &DuxPaths,
    sessions: &[AgentSession],
    requested: &Path,
    delete_branch: bool,
) -> Result<RemovalOutcome, String> {
    let repo_path = PathBuf::from(&project.path);
    let worktrees = git::list_worktrees(&repo_path).map_err(|e| format!("{e:#}"))?;
    let classified =
        crate::project_browser::classify_project_worktrees(project, paths, sessions, worktrees);
    match resolve_removal(classified, requested) {
        RemovalResolution::NotManaged => Ok(RemovalOutcome::NotManaged),
        RemovalResolution::Attached => Ok(RemovalOutcome::Attached),
        RemovalResolution::Removable { path, branch } => {
            match branch_to_delete(delete_branch, branch.as_deref()) {
                // The user asked for the branch too. `remove_worktree` deletes
                // the branch the worktree is on; there is no second, drifted
                // branch here, because a worktree with no agent has no record of
                // what it was born on.
                Some(branch) => {
                    let removed = git::remove_worktree(&repo_path, &path, branch, None)
                        .map_err(|e| format!("{e:#}"))?;
                    Ok(RemovalOutcome::Removed {
                        path,
                        branch: Some(BranchOutcome {
                            name: branch.to_string(),
                            deletion: removed.branch,
                        }),
                    })
                }
                // Either the request did not ask, or the worktree is detached
                // and there is no branch to delete. Worktree only.
                None => {
                    git::remove_worktree_keep_branch(&repo_path, &path)
                        .map_err(|e| format!("{e:#}"))?;
                    Ok(RemovalOutcome::Removed { path, branch: None })
                }
            }
        }
    }
}

/// What a surface says after a successful removal, derived from what actually
/// happened and never from the checkbox the request carried: `git branch -D`
/// refuses a branch checked out somewhere else, so "the user asked" and "the
/// branch is gone" come apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalReport {
    pub message: String,
    /// A branch git refused to delete is a warning, not a success: the worktree
    /// went and the branch did not.
    pub warning: bool,
}

/// The report for a successful removal.
///
/// The branch half is honest about WHY a branch survived. When the user left
/// the checkbox off, the branch is kept BY CHOICE, which is a different
/// sentence from the agent-delete path's provenance wording
/// ([`crate::model::BranchProvenance::kept_branches_note`], "existed before
/// this agent and was kept"): here nothing was inferred about the branch's
/// origin, the user simply did not ask for it. Do not reuse that helper on this
/// path; it would attribute a reason dux does not have.
///
/// The web composes its own version of this ladder client-side
/// (`lib/worktreeDelete.ts`), because its reply carries the branch outcome as
/// JSON and the toast is built in the browser. Same rungs, and the same
/// answers on the three that describe a branch, but NOT the same strings
/// throughout: the no-branch rung differs deliberately. This one says the
/// branch, if there was one, was kept because the caller did not ask for it,
/// which is the whole truth for a request that may or may not have named a
/// branch; the web's dialog knows whether the worktree had a branch before it
/// sent anything, so its toast simply says the branch is still there. Each
/// side pins its own strings in its own tests; treat neither as a copy of the
/// other.
pub fn removal_report(worktree_path: &str, branch: Option<&BranchOutcome>) -> RemovalReport {
    let Some(branch) = branch else {
        return RemovalReport {
            message: format!(
                "Removed the worktree at {worktree_path}. Its branch, if it had one, was kept: \
                 you did not ask for it."
            ),
            warning: false,
        };
    };
    match &branch.deletion {
        git::BranchDeletion::Deleted => RemovalReport {
            message: format!(
                "Removed the worktree at {worktree_path} and deleted its branch \"{}\".",
                branch.name
            ),
            warning: false,
        },
        git::BranchDeletion::AlreadyGone => RemovalReport {
            message: format!(
                "Removed the worktree at {worktree_path}. Its branch \"{}\" was already gone.",
                branch.name
            ),
            warning: false,
        },
        git::BranchDeletion::Refused { reason } => RemovalReport {
            message: format!(
                "Removed the worktree at {worktree_path}, but its branch is still there. {}",
                git::branch_refusal_note(&branch.name, reason)
            ),
            warning: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProjectBranchStatus, ProviderKind, SessionStatus};
    use chrono::Utc;
    use std::fs;

    fn project(root: &Path, repo: &Path) -> (Project, DuxPaths) {
        let project = Project {
            id: "project-1".to_string(),
            name: "demo".to_string(),
            path: repo.to_string_lossy().to_string(),
            explicit_default_provider: None,
            default_provider: ProviderKind::new("codex"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Leading,
            path_missing: false,
            created_at: None,
        };
        let paths = DuxPaths {
            root: root.to_path_buf(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("lock"),
        };
        (project, paths)
    }

    fn session(worktree: &Path) -> AgentSession {
        AgentSession {
            id: "session-1".to_string(),
            slot_tab_id: "session-1".to_string(),
            provider: ProviderKind::new("codex"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_focused_tab: None,
            workspace: crate::model::AgentWorkspace::Managed(crate::model::ManagedWorkspace {
                project_id: "project-1".to_string(),
                project_path: None,
                source_branch: "main".to_string(),
                branch_name: "held".to_string(),
                initial_branch: "held".to_string(),
                branch_provenance: crate::model::BranchProvenance::CreatedByDux,
                worktree_path: worktree.to_string_lossy().to_string(),
            }),
        }
    }

    /// A classification with four worktrees: the project checkout, a free
    /// managed one, a managed one an agent holds, and an external one.
    fn classified(root: &Path) -> (PathBuf, PathBuf, PathBuf, Vec<ProjectWorktreeEntry>) {
        let repo = root.join("repo");
        let free = root.join("worktrees").join("demo").join("free");
        let held = root.join("worktrees").join("demo").join("held");
        let external = root.join("external");
        for dir in [&repo, &free, &held, &external] {
            fs::create_dir_all(dir).unwrap();
        }
        let (project, paths) = project(root, &repo);
        let sessions = vec![session(&held)];
        let worktrees = vec![
            git::GitWorktree {
                path: repo.clone(),
                head: Some("0000000".to_string()),
                branch_name: Some("main".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: free.clone(),
                head: Some("1111111".to_string()),
                branch_name: Some("free".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: held.clone(),
                head: Some("2222222".to_string()),
                branch_name: Some("held".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: external.clone(),
                head: Some("3333333".to_string()),
                branch_name: Some("feature".to_string()),
                detached: false,
            },
        ];
        let entries = crate::project_browser::classify_project_worktrees(
            &project, &paths, &sessions, worktrees,
        );
        (
            free.canonicalize().unwrap(),
            held.canonicalize().unwrap(),
            external.canonicalize().unwrap(),
            entries,
        )
    }

    #[test]
    fn the_manager_lists_managed_worktrees_only() {
        let root = tempfile::tempdir().unwrap();
        let (free, held, _external, entries) = classified(root.path());
        let rows = manageable_worktrees(entries);
        let paths = rows.iter().map(|row| row.path.clone()).collect::<Vec<_>>();
        assert!(paths.contains(&free), "the free managed worktree is listed");
        assert!(
            paths.contains(&held),
            "an attached worktree is still listed, marked rather than hidden"
        );
        assert_eq!(
            paths.len(),
            2,
            "the project checkout and the external worktree are not the manager's"
        );
    }

    #[test]
    fn an_attached_worktree_is_listed_but_not_removable() {
        let root = tempfile::tempdir().unwrap();
        let (free, held, _external, entries) = classified(root.path());
        let rows = manageable_worktrees(entries);
        let free_row = rows.iter().find(|row| row.path == free).unwrap();
        let held_row = rows.iter().find(|row| row.path == held).unwrap();
        assert!(free_row.is_removable());
        assert_eq!(free_row.branch.as_deref(), Some("free"));
        assert!(!held_row.is_removable());
        assert_eq!(held_row.attached_session_id.as_deref(), Some("session-1"));
    }

    #[test]
    fn resolving_a_removal_answers_the_three_cases() {
        let root = tempfile::tempdir().unwrap();
        let (free, held, external, entries) = classified(root.path());
        assert_eq!(
            resolve_removal(entries.clone(), &free),
            RemovalResolution::Removable {
                path: free.clone(),
                branch: Some("free".to_string()),
            }
        );
        assert_eq!(
            resolve_removal(entries.clone(), &held),
            RemovalResolution::Attached
        );
        assert_eq!(
            resolve_removal(entries.clone(), &external),
            RemovalResolution::NotManaged,
            "an external worktree is not the manager's to remove"
        );
        assert_eq!(
            resolve_removal(entries, &root.path().join("nowhere")),
            RemovalResolution::NotManaged
        );
    }

    #[test]
    fn a_detached_worktree_resolves_with_no_branch() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let detached = root.path().join("worktrees").join("demo").join("loose");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&detached).unwrap();
        let (project, paths) = project(root.path(), &repo);
        let entries = crate::project_browser::classify_project_worktrees(
            &project,
            &paths,
            &[],
            vec![git::GitWorktree {
                path: detached.clone(),
                head: Some("abcdef0".to_string()),
                branch_name: None,
                detached: true,
            }],
        );
        assert_eq!(
            resolve_removal(entries, &detached),
            RemovalResolution::Removable {
                path: detached.canonicalize().unwrap(),
                branch: None,
            }
        );
    }

    #[test]
    fn a_branch_is_deleted_only_when_asked_for_and_present() {
        assert_eq!(branch_to_delete(true, Some("free")), Some("free"));
        assert_eq!(branch_to_delete(false, Some("free")), None);
        assert_eq!(
            branch_to_delete(true, None),
            None,
            "a detached worktree sends nothing however the checkbox was left"
        );
        assert_eq!(branch_to_delete(false, None), None);
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A real repository with one managed worktree on its own branch.
    fn real_repo(root: &Path) -> (Project, DuxPaths, PathBuf) {
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "--initial-branch=main"]);
        git_in(&repo, &["config", "user.email", "dux@example.com"]);
        git_in(&repo, &["config", "user.name", "dux"]);
        fs::write(repo.join("README.md"), "hi\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-m", "first"]);
        let (project, paths) = project(root, &repo);
        let worktree = paths.worktrees_root.join("demo").join("free");
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "free",
                worktree.to_string_lossy().as_ref(),
            ],
        );
        (project, paths, worktree)
    }

    fn branch_exists(repo: &Path, branch: &str) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn removing_a_worktree_without_the_branch_keeps_the_branch() {
        let root = tempfile::tempdir().unwrap();
        let (project, paths, worktree) = real_repo(root.path());
        let repo = PathBuf::from(&project.path);
        let outcome = remove_managed_worktree(&project, &paths, &[], &worktree, false).unwrap();
        assert!(matches!(
            outcome,
            RemovalOutcome::Removed { branch: None, .. }
        ));
        assert!(!worktree.exists(), "the worktree directory must be gone");
        assert!(branch_exists(&repo, "free"), "the branch must survive");
    }

    #[test]
    fn removing_a_worktree_with_the_branch_deletes_the_branch() {
        let root = tempfile::tempdir().unwrap();
        let (project, paths, worktree) = real_repo(root.path());
        let repo = PathBuf::from(&project.path);
        let outcome = remove_managed_worktree(&project, &paths, &[], &worktree, true).unwrap();
        let RemovalOutcome::Removed {
            branch: Some(branch),
            ..
        } = outcome
        else {
            panic!("expected a removal that touched the branch, got {outcome:?}");
        };
        assert_eq!(branch.name, "free");
        assert_eq!(branch.deletion, git::BranchDeletion::Deleted);
        assert!(!worktree.exists());
        assert!(!branch_exists(&repo, "free"), "the branch must be gone");
    }

    #[test]
    fn removing_an_attached_worktree_is_refused_and_touches_nothing() {
        let root = tempfile::tempdir().unwrap();
        let (project, paths, worktree) = real_repo(root.path());
        let sessions = vec![session(&worktree)];
        let outcome =
            remove_managed_worktree(&project, &paths, &sessions, &worktree, true).unwrap();
        assert_eq!(outcome, RemovalOutcome::Attached);
        assert!(worktree.exists(), "the attached worktree must survive");
        assert!(branch_exists(Path::new(&project.path), "free"));
    }

    #[test]
    fn the_report_says_what_happened_to_the_branch() {
        let kept = removal_report("/tmp/wt", None);
        assert!(!kept.warning);
        assert!(
            kept.message.contains("you did not ask for it"),
            "a kept branch is kept BY CHOICE here, not by provenance: {}",
            kept.message
        );

        let deleted = removal_report(
            "/tmp/wt",
            Some(&BranchOutcome {
                name: "free".to_string(),
                deletion: git::BranchDeletion::Deleted,
            }),
        );
        assert!(!deleted.warning);
        assert!(deleted.message.contains("deleted its branch \"free\""));

        let gone = removal_report(
            "/tmp/wt",
            Some(&BranchOutcome {
                name: "free".to_string(),
                deletion: git::BranchDeletion::AlreadyGone,
            }),
        );
        assert!(!gone.warning);
        assert!(gone.message.contains("was already gone"));

        let refused = removal_report(
            "/tmp/wt",
            Some(&BranchOutcome {
                name: "free".to_string(),
                deletion: git::BranchDeletion::Refused {
                    reason: "error: cannot delete branch 'free' used by worktree".to_string(),
                },
            }),
        );
        assert!(refused.warning, "a surviving branch is not a clean success");
        assert!(refused.message.contains("still there"));
        assert!(
            refused.message.contains("git branch -D \"free\""),
            "the report names the way out: {}",
            refused.message
        );
    }
}
