//! Project-browser and project-worktree intelligence helpers. Pure free
//! functions used by the project-browser worker (`spawn_browser_entries`),
//! the worktree picker (`spawn_project_worktrees_worker`), and the project
//! branch-status worker (`spawn_project_branch_status_checks`). The spawn
//! fns themselves move to `Engine` in T3b once these helpers are in core.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use crate::config::{Config, DuxPaths, ProjectConfig, expand_path};
use crate::git::{self, GitWorktree};
use crate::model::{AgentSession, Project, ProjectBranchStatus, ProviderKind};
use crate::worker::{
    BranchWarningKind, BrowserEntry, NonDefaultBranchAction, ProjectWorktreeEntry, WorkerEvent,
};

pub fn browser_entries(dir: &Path) -> Vec<BrowserEntry> {
    let mut entries = fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let is_git_repo = path.join(".git").exists();
            let label = if is_git_repo {
                name
            } else {
                format!("{name}/")
            };
            Some(BrowserEntry {
                is_git_repo,
                path,
                label,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        b.is_git_repo
            .cmp(&a.is_git_repo)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    if let Some(parent) = dir.parent() {
        entries.insert(
            0,
            BrowserEntry {
                path: parent.to_path_buf(),
                label: "../".to_string(),
                is_git_repo: false,
            },
        );
    }
    entries
}

/// Resolve the directory the add-project browser should open at, from config.
///
/// Prefers `[defaults] start_directory` when it is set AND currently a directory
/// (a stale/typo'd path falls through rather than dead-ending the picker on a
/// non-existent dir), then `$HOME`, then the process working directory, then `.`.
/// Shared by the TUI picker and the web `/api/v1/browse` fallback so both honor
/// the same configured value and the same fallback chain. Reads `config` live, so
/// a reload that swaps `engine.config` is reflected on the next call.
pub fn resolve_start_dir(config: &Config) -> PathBuf {
    config
        .defaults
        .start_directory
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        })
}

pub(crate) fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Resolve the leading branch for a project. Prefer the remote's default
/// branch (origin/HEAD) when available, otherwise fall back to whatever
/// branch is currently checked out. When no current branch is known (detached
/// HEAD), falls back to "main". Pure helper -- only touches git plumbing.
pub fn leading_branch_for_project(path: &Path, current_branch: Option<&str>) -> String {
    match git::remote_default_branch(path) {
        Some(default) => default,
        // No remote default: use the current branch when available, else
        // fall back to "main" (the same heuristic used in load_projects).
        None => current_branch
            .map(|s| s.to_string())
            .unwrap_or_else(|| "main".to_string()),
    }
}

/// Convert a slice of `ProjectConfig` entries (from SQLite) into runtime `Project` values.
/// Each project gets its path expanded, its provider resolved (falling back to the global
/// default), and its current branch read from git. Missing or non-git paths are flagged
/// with `path_missing = true` and receive an empty `current_branch`.
pub fn load_projects(
    project_configs: &[ProjectConfig],
    created_ats: &HashMap<String, chrono::DateTime<chrono::Utc>>,
    config: &Config,
) -> Vec<Project> {
    let mut projects = Vec::new();
    for project in project_configs {
        let (path, missing) = match expand_path(&project.path) {
            Some(expanded) => {
                let p = PathBuf::from(&expanded);
                let missing = !p.exists() || !git::is_git_repo(&p);
                (p, missing)
            }
            None => {
                // Unsafe or invalid path – treat as missing.
                (PathBuf::from(&project.path), true)
            }
        };
        let provider = project
            .default_provider
            .as_deref()
            .map(ProviderKind::from_str)
            .unwrap_or_else(|| config.default_provider());
        let current_branch = if missing {
            String::new()
        } else {
            git::current_branch_opt(&path)
                .ok()
                .flatten()
                .unwrap_or_default()
        };
        let leading_branch = project.leading_branch.clone().or_else(|| {
            (!missing).then(|| {
                leading_branch_for_project(
                    &path,
                    (!current_branch.is_empty()).then_some(current_branch.as_str()),
                )
            })
        });
        projects.push(Project {
            id: project.id.clone(),
            name: project.name.clone().unwrap_or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("project")
                    .to_string()
            }),
            path: path.to_string_lossy().to_string(),
            explicit_default_provider: project
                .default_provider
                .as_deref()
                .map(ProviderKind::from_str),
            default_provider: provider,
            leading_branch,
            auto_reopen_agents: project.auto_reopen_agents,
            startup_command: project.startup_command.clone(),
            env: project.env.clone(),
            current_branch,
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: missing,
            created_at: created_ats.get(&project.id).copied(),
        });
    }
    projects
}

pub fn classify_project_worktrees(
    project: &Project,
    paths: &DuxPaths,
    sessions: &[AgentSession],
    worktrees: Vec<GitWorktree>,
) -> Vec<ProjectWorktreeEntry> {
    let managed_project_root = paths.worktrees_root.join(&project.name);
    let project_checkout_path = canonical_or_original(Path::new(&project.path));
    let session_by_path = sessions
        .iter()
        .map(|session| {
            (
                canonical_or_original(Path::new(&session.worktree_path)),
                session.id.clone(),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut entries = worktrees
        .into_iter()
        .map(|worktree| {
            let canonical_path = canonical_or_original(&worktree.path);
            let existing_session_id = session_by_path.get(&canonical_path).cloned();
            let is_project_checkout = canonical_path == project_checkout_path;
            let is_managed_by_dux = git::is_under(&managed_project_root, &worktree.path);
            let is_external = !is_managed_by_dux;
            let is_selectable = existing_session_id.is_none() && !is_project_checkout;
            ProjectWorktreeEntry {
                path: canonical_path,
                branch_name: worktree.label(),
                is_managed_by_dux,
                existing_session_id,
                is_external,
                is_project_checkout,
                is_selectable,
            }
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| {
        a.is_selectable
            .cmp(&b.is_selectable)
            .reverse()
            .then_with(|| a.is_project_checkout.cmp(&b.is_project_checkout))
            .then_with(|| {
                a.branch_name
                    .to_lowercase()
                    .cmp(&b.branch_name.to_lowercase())
            })
            .then_with(|| a.path.cmp(&b.path))
    });
    entries
}

pub fn run_project_branch_status_job(project: Project, worker_tx: Sender<WorkerEvent>) {
    let repo_path = PathBuf::from(&project.path);
    // Use current_branch_opt so a detached HEAD is treated as "no current
    // branch" (empty string) rather than a hard error.
    let result = git::current_branch_opt(&repo_path)
        .map(|opt_branch| {
            let branch = opt_branch.unwrap_or_default();
            let branch_status = if let Some(leading_branch) = project.leading_branch.as_deref() {
                if branch == leading_branch {
                    ProjectBranchStatus::Leading
                } else {
                    ProjectBranchStatus::NotLeading
                }
            } else {
                let warning_kind = git::branch_warning_kind(&repo_path, &branch);
                git::branch_status_from_warning(warning_kind.as_ref())
            };
            (branch, branch_status)
        })
        .map_err(|err| format!("{err:#}"));
    let _ = worker_tx.send(WorkerEvent::ProjectBranchStatusReady {
        project_id: project.id,
        result,
    });
}

pub fn run_checkout_project_default_branch_inspection_job(
    project: Project,
    worker_tx: Sender<WorkerEvent>,
    status_op_id: Option<String>,
) {
    let repo_path = PathBuf::from(&project.path);
    // Use current_branch_opt so a detached HEAD is treated as "no current
    // branch" (empty string) rather than a hard error.
    let result = git::current_branch_opt(&repo_path)
        .map(|opt_branch| {
            let branch = opt_branch.unwrap_or_default();
            let warning_kind = if let Some(leading_branch) = project.leading_branch.as_deref() {
                if branch == leading_branch {
                    None
                } else {
                    Some(BranchWarningKind::Known {
                        default_branch: leading_branch.to_string(),
                    })
                }
            } else {
                git::branch_warning_kind(&repo_path, &branch)
            };
            (branch, warning_kind)
        })
        .map_err(|err| format!("{err:#}"));
    let _ = worker_tx.send(WorkerEvent::CheckoutProjectDefaultBranchInspected {
        project,
        result,
        status_op_id,
    });
}

/// Background job for the second phase of the non-default-branch checkout flow:
/// runs `git switch <target_branch>` in the source repo and reports the outcome
/// via `WorkerEvent::NonDefaultBranchCheckoutCompleted` so the caller can
/// continue the selected action or surface the git error. Used by both the TUI
/// (Add Project "switch first" and checkout-project-default-branch) and the web
/// engine actor's `drive_checkout_followup`, so the worker-2 spawn logic is
/// shared rather than duplicated per surface.
pub fn run_add_project_checkout_job(
    action: NonDefaultBranchAction,
    target_branch: String,
    worker_tx: Sender<WorkerEvent>,
    status_op_id: Option<String>,
) {
    let path = action.repo_path().to_string();
    let result = git::switch_branch(Path::new(&path), &target_branch).map_err(|e| format!("{e:#}"));
    let _ = worker_tx.send(WorkerEvent::NonDefaultBranchCheckoutCompleted {
        action,
        target_branch,
        result,
        status_op_id,
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;

    use chrono::Utc;
    use tempfile::tempdir;

    use super::*;
    use crate::config::Config;
    use crate::model::{ProviderKind, SessionStatus};

    #[test]
    fn resolve_start_dir_prefers_an_existing_configured_directory() {
        let dir = tempdir().expect("start tempdir");
        let mut config = Config::default();
        config.defaults.start_directory = Some(dir.path().to_string_lossy().to_string());
        assert_eq!(resolve_start_dir(&config), dir.path());
    }

    #[test]
    fn resolve_start_dir_falls_back_when_configured_path_is_missing() {
        let missing = tempdir().expect("missing tempdir");
        let missing_path = missing.path().join("does-not-exist");
        let mut config = Config::default();
        config.defaults.start_directory = Some(missing_path.to_string_lossy().to_string());
        // A non-existent configured path must not be returned; it falls through to
        // the home/cwd chain. We only assert it did NOT echo the missing path back.
        assert_ne!(resolve_start_dir(&config), missing_path);
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn checkout_project_default_branch_inspection_uses_stored_leading_branch() {
        let repo = tempdir().expect("repo tempdir");
        run_git(repo.path(), &["init", "-b", "trunk"]);
        run_git(repo.path(), &["config", "user.name", "test"]);
        run_git(repo.path(), &["config", "user.email", "t@t"]);
        run_git(repo.path(), &["commit", "--allow-empty", "-m", "init"]);
        run_git(repo.path(), &["switch", "-c", "feature"]);

        let project = Project {
            id: "project-1".to_string(),
            name: "demo".to_string(),
            path: repo.path().to_string_lossy().to_string(),
            explicit_default_provider: None,
            default_provider: ProviderKind::from_str("codex"),
            leading_branch: Some("trunk".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "feature".to_string(),
            branch_status: ProjectBranchStatus::NotLeading,
            path_missing: false,
            created_at: None,
        };
        let (worker_tx, worker_rx) = mpsc::channel();

        run_checkout_project_default_branch_inspection_job(project, worker_tx, None);

        match worker_rx.recv().expect("worker event") {
            WorkerEvent::CheckoutProjectDefaultBranchInspected { result, .. } => {
                let (current_branch, warning_kind) = result.expect("inspection");
                assert_eq!(current_branch, "feature");
                assert!(matches!(
                    warning_kind,
                    Some(BranchWarningKind::Known { default_branch }) if default_branch == "trunk"
                ));
            }
            _ => panic!("expected checkout inspection event"),
        }
    }

    #[test]
    fn classify_project_worktrees_marks_managed_external_and_existing_agent() {
        let root =
            std::env::temp_dir().join(format!("dux-classify-worktrees-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let managed = root.join("worktrees").join("demo").join("managed-orphan");
        let external = root.join("external checkout");
        let existing = root.join("worktrees").join("demo").join("existing-agent");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&managed).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::create_dir_all(&existing).unwrap();

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
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("lock"),
        };
        let sessions = vec![AgentSession {
            id: "session-1".to_string(),
            project_id: project.id.clone(),
            project_path: Some(project.path.clone()),
            provider: ProviderKind::new("codex"),
            source_branch: "main".to_string(),
            branch_name: "existing".to_string(),
            worktree_path: existing.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }];
        let worktrees = vec![
            git::GitWorktree {
                path: repo.clone(),
                head: Some("0000000".to_string()),
                branch_name: Some("main".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: managed.clone(),
                head: Some("1111111".to_string()),
                branch_name: Some("managed-orphan".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: external.clone(),
                head: Some("2222222".to_string()),
                branch_name: Some("feature".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: existing.clone(),
                head: Some("3333333".to_string()),
                branch_name: Some("existing".to_string()),
                detached: false,
            },
        ];

        let entries = classify_project_worktrees(&project, &paths, &sessions, worktrees);
        let managed_entry = entries
            .iter()
            .find(|entry| entry.path == managed.canonicalize().unwrap())
            .unwrap();
        assert!(managed_entry.is_managed_by_dux);
        assert!(!managed_entry.is_external);
        assert!(managed_entry.is_selectable);

        let external_entry = entries
            .iter()
            .find(|entry| entry.path == external.canonicalize().unwrap())
            .unwrap();
        assert!(!external_entry.is_managed_by_dux);
        assert!(external_entry.is_external);
        assert!(external_entry.is_selectable);

        let existing_entry = entries
            .iter()
            .find(|entry| entry.path == existing.canonicalize().unwrap())
            .unwrap();
        assert_eq!(
            existing_entry.existing_session_id.as_deref(),
            Some("session-1")
        );
        assert!(!existing_entry.is_selectable);

        let project_checkout_entry = entries
            .iter()
            .find(|entry| entry.path == repo.canonicalize().unwrap())
            .unwrap();
        assert!(project_checkout_entry.is_project_checkout);
        assert!(!project_checkout_entry.is_selectable);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_projects_converts_project_config_to_project() {
        use crate::config::{Config, ProjectConfig};

        let cfg = ProjectConfig {
            id: "test-project-id".to_string(),
            path: "/nonexistent/path/that/does/not/exist".to_string(),
            name: Some("my-project".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
        };
        let config = Config::default();
        let mut created_ats = HashMap::new();
        let added = Utc::now();
        created_ats.insert("test-project-id".to_string(), added);
        let projects = load_projects(&[cfg], &created_ats, &config);

        assert_eq!(projects.len(), 1);
        let project = &projects[0];
        assert_eq!(project.id, "test-project-id");
        // created_at is threaded from the store row map keyed by project id.
        assert_eq!(project.created_at, Some(added));
        // No explicit provider → falls back to the global default ("claude").
        assert_eq!(
            project.default_provider.as_str(),
            config.defaults.provider.as_str()
        );
        // Missing path → branch_status is Unknown.
        assert!(matches!(
            project.branch_status,
            ProjectBranchStatus::Unknown
        ));
        // Missing path → path_missing is true.
        assert!(project.path_missing);
    }

    #[test]
    fn leading_branch_for_project_returns_main_when_detached_and_no_remote_default() {
        // A non-git directory: remote_default_branch returns None, current
        // branch is None (detached). Must fall back to "main".
        let tmp = tempdir().unwrap();
        let result = leading_branch_for_project(tmp.path(), None);
        assert_eq!(result, "main");
    }

    #[test]
    fn leading_branch_for_project_returns_current_branch_when_no_remote_default() {
        // No remote default: should return whatever branch was passed.
        let tmp = tempdir().unwrap();
        let result = leading_branch_for_project(tmp.path(), Some("feature/my-thing"));
        assert_eq!(result, "feature/my-thing");
    }
}
