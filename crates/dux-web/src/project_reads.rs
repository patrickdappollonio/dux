//! REST reads scoped to a single project (Phase 6 of the REST-first migration).
//! These used to ride the retired `/ws` request/reply pairs
//! (`list_project_worktrees` → `project_worktrees`, `inspect_project_path` →
//! `project_path_inspection`); they are now plain authenticated GETs.
//!
//! - `GET /api/v1/projects/:id/worktrees` — the project's adoptable managed
//!   worktree candidates for the "Attach worktree" picker. 404 for an unknown
//!   project id.
//! - `GET /api/v1/projects/inspect?path=` — branch pre-flight for the add-project
//!   flow: the candidate repo's current branch + a non-default-branch warning.
//!   400 for an empty/relative path (the path must be absolute — it is not a
//!   registered project yet, so it is inspected straight off the filesystem).
//!
//! Both shell to git, so the classification/inspection runs OFF the async reactor
//! (`spawn_blocking`), following the old handlers' precedent. Merged into the
//! authenticated (gated) sub-router in `server.rs`, so an unauthenticated request
//! 401s before reaching here.
//!
//! NOTE: `/api/v1/projects/inspect` (a static segment) coexists with
//! `/api/v1/projects/:id` (the parameterized PATCH/DELETE in
//! [`crate::project_actions`]) — axum's matcher prefers the static segment, the
//! same way `/api/v1/projects/reorder` already does.

use std::path::Path;

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::rest_common::id_within_bound;
use crate::server::AppState;

/// Upper bound on the `?path=` query value before any filesystem touch (matches
/// the bound used by the directory browser).
const MAX_PATH_LEN: usize = 4096;

/// The gated project read routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects/inspect", get(inspect_path))
        .route("/api/v1/projects/{id}/worktrees", get(list_worktrees))
}

// ── Worktrees ──────────────────────────────────────────────────────────────────

/// A managed-worktree candidate, mirroring the frontend's
/// `ProjectWorktreeEntryView` (`projectsApi.ts` / `types.ts`).
#[derive(Serialize)]
struct ProjectWorktreeEntryView {
    worktree_path: String,
    branch_name: String,
    adoptable: bool,
    reason: Option<String>,
}

#[derive(Serialize)]
struct WorktreesReply {
    entries: Vec<ProjectWorktreeEntryView>,
}

async fn list_worktrees(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    if !id_within_bound(&id) {
        return (StatusCode::NOT_FOUND, "unknown project").into_response();
    }
    // Resolve the project + classification inputs from the engine (an instant
    // lookup), then classify off-thread: classification shells to git, so it must
    // not run on the engine loop or the async reactor (the browse precedent).
    match state.engine.project_worktree_inputs(id).await {
        None => (StatusCode::NOT_FOUND, "unknown project").into_response(),
        Some((project, paths, sessions)) => {
            match tokio::task::spawn_blocking(move || {
                classify_managed_worktrees(&project, &paths, &sessions)
            })
            .await
            {
                Ok(Ok(entries)) => Json(WorktreesReply { entries }).into_response(),
                Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("worktree listing failed: {e}"),
                )
                    .into_response(),
            }
        }
    }
}

/// Classify a project's git worktrees and project the MANAGED ones (under dux's
/// worktrees root) into wire-safe entries. External worktrees and the project
/// checkout are excluded — they are not part of the managed-adoption flow. Each
/// managed entry is marked adoptable when it has no live agent; otherwise the
/// reason ("Already has an agent.") is surfaced so the client can disable it.
///
/// Runs in `spawn_blocking`: `list_worktrees` shells to git. Returns a
/// user-facing error string when the git listing fails.
fn classify_managed_worktrees(
    project: &dux_core::model::Project,
    paths: &dux_core::config::DuxPaths,
    sessions: &[dux_core::model::AgentSession],
) -> Result<Vec<ProjectWorktreeEntryView>, String> {
    let worktrees =
        dux_core::git::list_worktrees(Path::new(&project.path)).map_err(|e| format!("{e:#}"))?;
    let entries =
        dux_core::project_browser::classify_project_worktrees(project, paths, sessions, worktrees)
            .into_iter()
            .filter(|entry| entry.is_managed_by_dux && !entry.is_project_checkout)
            .map(|entry| ProjectWorktreeEntryView {
                worktree_path: entry.path.to_string_lossy().to_string(),
                branch_name: entry.branch_name,
                adoptable: entry.is_selectable,
                reason: if entry.is_selectable {
                    None
                } else {
                    Some("Already has an agent.".to_string())
                },
            })
            .collect();
    Ok(entries)
}

// ── Inspect ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct InspectQuery {
    #[serde(default)]
    path: String,
}

/// The branch-warning classification, mirroring the frontend's `BranchWarningView`
/// (`{ kind: "known", default_branch } | { kind: "heuristic" }`).
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BranchWarningView {
    Known { default_branch: String },
    Heuristic,
}

#[derive(Serialize)]
struct InspectReply {
    /// How the path classifies for the add flow: `"repo"` (work-tree root),
    /// `"bare"` (bare root), `"repo_subdir"` (inside a repo or inside git's
    /// internal directory; blocked client-side), or `"plain"` (not a repo; the
    /// client offers to initialize one). Old bundles never see the new kinds
    /// because they only inspect rows they already believe are repos; a new
    /// bundle treats a missing `kind` as `"repo"`.
    kind: &'static str,
    /// The enclosing repository root, for the `repo_subdir` kind. `None` when
    /// the path is inside git's internal directory (no user-facing root to
    /// name) and for every other kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo_root: Option<String>,
    /// For the `plain` kind: names of starter-.gitignore candidate directories
    /// present in the folder, so the client can say what a seed would cover.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    gitignore_candidates: Vec<String>,
    current_branch: Option<String>,
    warning: Option<BranchWarningView>,
    /// `false` for a freshly `git init`'d repo with an unborn HEAD (no
    /// commits). The UI uses this to offer creating an initial commit before
    /// the repo can back worktrees. NOTE: this is `repo_has_commits`'s fail-open
    /// bool, so a transient git failure also yields `false` — acceptable for a
    /// read-only hint (the mutating add path independently re-checks with the
    /// fail-closed `repo_commit_state` and never double-commits).
    has_commits: bool,
}

async fn inspect_path(
    State(_state): State<AppState>,
    Query(query): Query<InspectQuery>,
) -> Response {
    let path = query.path;
    // The path is inspected straight off the filesystem (it is not a registered
    // project yet), so it must be an absolute path. Reject empty/relative with 400.
    if path.is_empty() {
        return (StatusCode::BAD_REQUEST, "path is required").into_response();
    }
    if !Path::new(&path).is_absolute() {
        return (StatusCode::BAD_REQUEST, "path must be absolute").into_response();
    }
    if path.chars().count() > MAX_PATH_LEN {
        return (StatusCode::BAD_REQUEST, "path is too long").into_response();
    }

    // Pre-flight branch inspection mirroring the TUI's `add_project`: it runs
    // `current_branch_opt` then `branch_warning_kind` before the non-default-branch
    // prompt. Both are bounded git plumbing reads with no working-tree writes, so
    // this runs off the async reactor in `spawn_blocking` (the browse precedent).
    // A detached HEAD yields `current_branch: null` in the response with no warning
    // (the caller cannot switch the user to a default branch from a detached state).
    // A non-repo path still fails with a non-Ok result, which is returned as 400.
    let result = tokio::task::spawn_blocking(move || {
        let repo = Path::new(&path);
        // Classify first so the add flow can distinguish a plain folder (offer
        // init), a repo subfolder / git-internal dir (blocked), and a bare or
        // work-tree root (the existing probes). Indeterminate falls through to
        // the probes, whose error becomes the 400 it always was.
        let kind = dux_core::git::repo_path_kind(repo);
        match kind {
            dux_core::git::RepoPathKind::NotARepo => {
                let gitignore_candidates = dux_core::gitignore_seed::matched_candidates(repo)
                    .iter()
                    .map(|c| c.dir.to_string())
                    .collect();
                return Ok(InspectReply {
                    kind: "plain",
                    repo_root: None,
                    gitignore_candidates,
                    current_branch: None,
                    warning: None,
                    has_commits: false,
                });
            }
            dux_core::git::RepoPathKind::InsideWorkTree { root } => {
                return Ok(InspectReply {
                    kind: "repo_subdir",
                    repo_root: Some(root.to_string_lossy().to_string()),
                    gitignore_candidates: Vec::new(),
                    current_branch: None,
                    warning: None,
                    has_commits: true,
                });
            }
            dux_core::git::RepoPathKind::InsideGitDir { .. } => {
                // Same blocked treatment client-side; the panel copy degrades
                // to not naming a root.
                return Ok(InspectReply {
                    kind: "repo_subdir",
                    repo_root: None,
                    gitignore_candidates: Vec::new(),
                    current_branch: None,
                    warning: None,
                    has_commits: true,
                });
            }
            dux_core::git::RepoPathKind::BareRoot
            | dux_core::git::RepoPathKind::WorkTreeRoot
            | dux_core::git::RepoPathKind::Indeterminate => {}
        }
        let reply_kind = match kind {
            dux_core::git::RepoPathKind::BareRoot => "bare",
            _ => "repo",
        };
        let branch = dux_core::git::current_branch_opt(repo).map_err(|e| format!("{e:#}"))?;
        let has_commits = dux_core::git::repo_has_commits(repo);
        // Derive the branch WARNING from the CORE-owned `add_project_plan` (the
        // single-source decision the TUI's add_project also consumes, pinned by
        // the shared vector matrix). The reply `kind` still comes from
        // `RepoPathKind` (bare vs repo) and `has_commits` drives the client's
        // initial-commit offer; only the warning selection is the shared
        // decision. A detached HEAD carries no branch_warning.
        let branch_warning = branch
            .as_deref()
            .and_then(|b| dux_core::git::branch_warning_kind(repo, b));
        let inspection = dux_core::add_project_plan::AddProjectInspection {
            path_kind: kind.clone(),
            current_branch: branch.clone(),
            branch_warning,
            has_commits,
        };
        let warning = match dux_core::add_project_plan::add_project_plan(&inspection).warning {
            dux_core::add_project_plan::AddProjectWarning::NotOnDefaultBranch {
                default_branch,
            } => Some(BranchWarningView::Known { default_branch }),
            dux_core::add_project_plan::AddProjectWarning::NotOnDefaultBranchUnknown => {
                Some(BranchWarningView::Heuristic)
            }
            dux_core::add_project_plan::AddProjectWarning::None => None,
        };
        Ok::<_, String>(InspectReply {
            kind: reply_kind,
            repo_root: None,
            gitignore_candidates: Vec::new(),
            current_branch: branch,
            warning,
            has_commits,
        })
    })
    .await;

    match result {
        Ok(Ok(reply)) => Json(reply).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("inspection failed: {e}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::test_support::router_no_auth;

    /// Initialize a git repo on `main` with one commit so `current_branch`
    /// resolves and there is no `origin/HEAD` (the heuristic-warning path).
    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
    }

    #[tokio::test]
    async fn inspect_reports_current_branch() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let path = repo.path().to_string_lossy().to_string();

        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/inspect?path={path}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["current_branch"], "main");
        // On `main` with no origin, there is no warning.
        assert!(value["warning"].is_null());
    }

    #[tokio::test]
    async fn inspect_rejects_empty_path_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/inspect?path=")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn inspect_rejects_relative_path_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/inspect?path=relative/dir")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// Build a detached-HEAD repo: init on `main`, commit once, then detach.
    fn init_repo_detached(dir: &Path) {
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        // Detach HEAD at the current commit.
        run(&["checkout", "--detach"]);
    }

    #[tokio::test]
    async fn inspect_detached_head_reports_null_branch_200() {
        let repo = tempfile::tempdir().unwrap();
        init_repo_detached(repo.path());
        let path = repo.path().to_string_lossy().to_string();

        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/inspect?path={path}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Detached HEAD: branch must be JSON null and no warning emitted.
        assert!(
            value["current_branch"].is_null(),
            "expected null current_branch, got {value}"
        );
        assert!(
            value["warning"].is_null(),
            "expected null warning, got {value}"
        );
    }

    /// Init a repo with `git init` but NO commit (unborn HEAD).
    fn init_repo_no_commit(dir: &Path) {
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "Test"]);
    }

    #[tokio::test]
    async fn inspect_reports_has_commits_true_for_repo_with_commit() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let path = repo.path().to_string_lossy().to_string();
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/inspect?path={path}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["has_commits"], true, "got {value}");
    }

    #[tokio::test]
    async fn inspect_reports_no_commits_for_unborn_repo() {
        let repo = tempfile::tempdir().unwrap();
        init_repo_no_commit(repo.path());
        let path = repo.path().to_string_lossy().to_string();
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/inspect?path={path}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // An unborn repo is still a valid git repo, so inspect succeeds (200)
        // and simply reports has_commits: false so the UI can offer to create
        // the initial commit.
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["has_commits"], false, "got {value}");
    }

    async fn inspect_json(path: &str) -> (StatusCode, serde_json::Value) {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/inspect?path={path}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn inspect_classifies_a_plain_folder_with_candidates() {
        // Was a 400; the adopt-a-folder flow now classifies a non-repo as
        // `kind: "plain"` and names the starter-.gitignore candidates so the
        // client can offer to initialize it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("node_modules")).unwrap();
        let (status, value) = inspect_json(&dir.path().to_string_lossy()).await;
        assert_eq!(status, StatusCode::OK, "got {value}");
        assert_eq!(value["kind"], "plain");
        assert_eq!(value["has_commits"], false);
        let candidates: Vec<&str> = value["gitignore_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(candidates, vec!["node_modules"]);
    }

    #[tokio::test]
    async fn inspect_classifies_repo_subdirs_and_git_dirs_as_blocked() {
        // Catches the client offering add (or init) on a folder inside a repo:
        // the server is the authority over the picker's `.git`-existence label.
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let sub = repo.path().join("src");
        std::fs::create_dir(&sub).unwrap();

        let (status, value) = inspect_json(&sub.to_string_lossy()).await;
        assert_eq!(status, StatusCode::OK, "got {value}");
        assert_eq!(value["kind"], "repo_subdir");
        assert_eq!(
            value["repo_root"].as_str().unwrap(),
            repo.path().canonicalize().unwrap().to_string_lossy()
        );

        let git_dir = repo.path().join(".git");
        let (status, value) = inspect_json(&git_dir.to_string_lossy()).await;
        assert_eq!(status, StatusCode::OK, "got {value}");
        assert_eq!(value["kind"], "repo_subdir");
        assert!(
            value.get("repo_root").is_none() || value["repo_root"].is_null(),
            "a git-internal dir names no user-facing root, got {value}"
        );
    }

    #[tokio::test]
    async fn inspect_classifies_a_bare_root_with_branch_fields() {
        // Catches the client offering `git init` on a bare repository.
        let bare = tempfile::tempdir().unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .current_dir(bare.path())
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok);
        let (status, value) = inspect_json(&bare.path().to_string_lossy()).await;
        assert_eq!(status, StatusCode::OK, "got {value}");
        assert_eq!(value["kind"], "bare");
        // The existing probes still run for a bare repo.
        assert_eq!(value["current_branch"], "main");
        assert_eq!(value["has_commits"], false);
    }

    #[tokio::test]
    async fn inspect_work_tree_root_reports_kind_repo() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let (status, value) = inspect_json(&repo.path().to_string_lossy()).await;
        assert_eq!(status, StatusCode::OK, "got {value}");
        assert_eq!(value["kind"], "repo");
        assert_eq!(value["current_branch"], "main");
    }

    #[tokio::test]
    async fn worktrees_404_for_unknown_project() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects/nope/worktrees")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
