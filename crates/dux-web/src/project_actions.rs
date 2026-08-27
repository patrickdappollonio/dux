//! REST write verbs for projects. Same
//! pattern as [`crate::session_actions`]: each handler derives a per-connection
//! [`StatusScope`] from the optional `X-Connection-Id` header and dispatches the
//! matching [`WireCommand`] via [`EngineHandle::apply_wire_scoped`].
//!
//! Every route is served plainly: dux has NO authentication, so none of these
//! ever 401s. The open access is deliberate (the single-tenant trusted-access
//! model in CLAUDE.md), and the app-wide guards are not authentication: a
//! Host-header allowlist stops a malicious web page rebinding DNS into this
//! server, and the same-origin check stops another site driving these verbs from a
//! visitor's browser, but a client sending no `Origin` (curl, a script) bypasses
//! it by design.
//!
//! Routes:
//! - `POST   /api/v1/projects`                 — add (body `{path, name?,
//!   checkout_default?}`); `Idempotency-Key` honored.
//! - `DELETE /api/v1/projects/:id`             — remove (does not touch the checkout).
//!   With `?delete_worktrees=true` it deletes the agents' worktrees too.
//! - `PATCH  /api/v1/projects/:id`             — update settings (provider /
//!   auto_reopen / startup_command / env), tri-state per field.
//! - `POST   /api/v1/projects/reorder`         — persist order (literal segment).
//! - `POST   /api/v1/projects/:id/pull`        — refresh the source checkout.
//! - `POST   /api/v1/projects/:id/checkout-default` — switch the checkout to default.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{patch, post},
};
use serde::{Deserialize, Serialize};

use dux_core::wire::WireCommand;

use crate::rest_common::{
    CREATE_AWAIT_TIMEOUT, await_new_project, id_within_bound, idempotency_key,
    provider_is_configured, scope_from_headers,
};
use crate::server::AppState;
use crate::session_actions::outcome_is_error;

/// The project-action routes. The literal `/reorder` segment is registered
/// alongside `:id`; axum's matcher prefers static segments over `:id`. (The
/// `GET /api/v1/projects` read lives in `workspace_routes`; axum merges the per-path
/// method routers, so `POST` here coexists with it.)
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects", post(add_project))
        .route("/api/v1/projects/reorder", post(reorder_projects))
        .route(
            "/api/v1/projects/{id}",
            patch(patch_project).delete(remove_project),
        )
        .route("/api/v1/projects/{id}/pull", post(pull_project))
        .route(
            "/api/v1/projects/{id}/checkout-default",
            post(checkout_default),
        )
}

// ── Add ──────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AddProjectBody {
    path: String,
    /// Display name; empty derives it from the path's basename.
    #[serde(default)]
    name: String,
    /// Check the repo's default branch out FIRST, then register it (mirrors the
    /// TUI's "Check Out & Add"). Only valid when the repo is on a non-default
    /// branch with a known default; the engine re-validates and rejects otherwise.
    #[serde(default)]
    checkout_default: bool,
    /// Create an empty initial commit BEFORE registering, so a freshly
    /// `git init`'d repo with an unborn HEAD can back worktrees. No-op (and
    /// harmless) if the repo already has commits. The user opts in via the
    /// add-project dialog after inspect reports `has_commits: false`.
    #[serde(default)]
    create_initial_commit: bool,
    /// Adopt a plain (non-repo) folder: run `git init`, seed a starter
    /// `.gitignore`, create an empty initial commit, then register. The user
    /// opts in via the add-project dialog after inspect reports
    /// `kind: "plain"`. The engine re-validates (the folder must not already
    /// be, or sit inside, a repository).
    #[serde(default)]
    init_repo: bool,
}

async fn add_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AddProjectBody>,
) -> Response {
    // Idempotency replay: a key that already produced a still-present project
    // returns it without adding another.
    let key = idempotency_key(&headers);
    if let Some(key) = &key
        && let Some(prev_id) = state.idempotency.get(key)
        && let Some(spine) = state.engine.spine().await
        && let Some(project) = spine.projects.into_iter().find(|p| p.id == prev_id)
    {
        return (StatusCode::OK, Json(project)).into_response();
    }

    let pre: std::collections::HashSet<String> = match state.engine.spine().await {
        Some(spine) => spine.projects.into_iter().map(|p| p.id).collect(),
        None => return engine_unavailable(),
    };

    // Pick the add variant, a strict precedence ladder: `init_repo` outranks
    // `create_initial_commit` (init subsumes the commit), which outranks
    // `checkout_default` (an unborn repo has no default branch to check out).
    // Like the checkout-default flow, the engine validates the path, serializes
    // per repo path, and runs the commit on a worker before registering — so the
    // mutating git work never runs on the async reactor here, and a failure (or
    // a repo that gained commits since inspect, which the handler registers as a
    // plain add) surfaces through the keyed status stream.
    let cmd = if body.init_repo {
        WireCommand::AddProjectInitRepo {
            path: body.path,
            name: body.name,
        }
    } else if body.create_initial_commit {
        WireCommand::AddProjectCreateInitialCommit {
            path: body.path,
            name: body.name,
        }
    } else if body.checkout_default {
        WireCommand::AddProjectCheckoutDefault {
            path: body.path,
            name: body.name,
        }
    } else {
        WireCommand::AddProject {
            path: body.path,
            name: body.name,
        }
    };

    match state
        .engine
        .apply_wire_scoped(cmd, scope_from_headers(&headers, &state.connections))
        .await
    {
        Ok(_) => {}
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    }

    // A direct add resolves synchronously (first poll wins); the checkout-default
    // add goes through a worker, so the poll covers it.
    match await_new_project(&state.engine, &pre, CREATE_AWAIT_TIMEOUT).await {
        Some(id) => {
            if let Some(key) = key {
                state.idempotency.record(key, id.clone());
            }
            let location = format!("/api/v1/projects/{id}");
            let body = match state.engine.spine().await {
                Some(spine) => match spine.projects.into_iter().find(|p| p.id == id) {
                    Some(project) => Json(project).into_response(),
                    None => Json(CreatedRef { id }).into_response(),
                },
                None => Json(CreatedRef { id }).into_response(),
            };
            (StatusCode::CREATED, [(header::LOCATION, location)], body).into_response()
        }
        None => StatusCode::ACCEPTED.into_response(),
    }
}

#[derive(Serialize)]
struct CreatedRef {
    id: String,
}

// ── Remove / Delete ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RemoveProjectQuery {
    /// When true, also remove every agent's worktree from disk (routes to the
    /// destructive `DeleteProject`). Defaults to false (keep the worktrees, plain
    /// `RemoveProject`) so a missing query parameter never deletes user data.
    #[serde(default)]
    delete_worktrees: bool,
}

async fn remove_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RemoveProjectQuery>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_project();
    }
    if !project_exists(&state, &id).await {
        return unknown_project();
    }
    // `delete_worktrees` selects the destructive variant, which cascades every
    // agent's worktree off disk; the default keeps the worktrees.
    let command = if q.delete_worktrees {
        WireCommand::DeleteProject { project_id: id }
    } else {
        WireCommand::RemoveProject { project_id: id }
    };
    match state
        .engine
        .apply_wire_scoped(command, scope_from_headers(&headers, &state.connections))
        .await
    {
        // An `Ok` error-toned status means the delete was REFUSED, not performed
        // (a tab is still launching, or an async worktree removal is in flight).
        // Report 409 with the message rather than a misleading 204 "deleted".
        Ok(outcome) => {
            if outcome_is_error(&outcome) {
                let msg = outcome
                    .status
                    .map(|s| s.message)
                    .unwrap_or_else(|| "project delete refused".to_string());
                return (StatusCode::CONFLICT, msg).into_response();
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Patch (settings) ─────────────────────────────────────────────────────────

/// Tri-state per-field project update: an absent field is untouched; a present
/// `null` clears the value (back to its default); a present value sets it. `env`
/// is a wholesale replace of the project's env map.
#[derive(Deserialize)]
struct PatchProjectBody {
    #[serde(default)]
    provider: Option<Option<String>>,
    #[serde(default)]
    auto_reopen_agents: Option<Option<bool>>,
    #[serde(default)]
    startup_command: Option<Option<String>>,
    #[serde(default)]
    env: Option<BTreeMap<String, String>>,
}

async fn patch_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PatchProjectBody>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_project();
    }
    if !project_exists(&state, &id).await {
        return unknown_project();
    }
    let scope = scope_from_headers(&headers, &state.connections);

    // Validate a provider SET up front, before dispatching any sub-command, so a bad
    // provider can never partially apply after auto-reopen/startup-command/env have
    // already committed (the PATCH dispatches each field as an independent wire
    // sub-command with no rollback). `provider` is tri-state: `Some(None)` clears it
    // (no validation needed); only `Some(Some(_))` sets a value to check. The engine
    // re-validates authoritatively. NOTE: the remaining fields stay non-atomic — a
    // later sub-command failing leaves earlier ones committed. That residual
    // non-atomicity across the independent fields is accepted: there is no engine
    // atomic-batch command, and the provider is the only field validated against the
    // configured list (the realistic failure mode), so guarding it up front removes
    // the partial-commit hazard that actually occurs in practice.
    if let Some(Some(provider)) = body.provider.as_ref() {
        match provider_is_configured(&state.engine, provider).await {
            Some(true) => {}
            Some(false) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Provider \"{provider}\" is not configured. Pick one of the configured providers."
                    ),
                )
                    .into_response();
            }
            None => return engine_unavailable(),
        }
    }

    if let Some(provider) = body.provider
        && let Err(e) = state
            .engine
            .apply_wire_scoped(
                WireCommand::UpdateProjectProvider {
                    project_id: id.clone(),
                    provider,
                },
                scope.clone(),
            )
            .await
    {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    if let Some(auto_reopen_agents) = body.auto_reopen_agents
        && let Err(e) = state
            .engine
            .apply_wire_scoped(
                WireCommand::UpdateProjectAutoReopen {
                    project_id: id.clone(),
                    auto_reopen_agents,
                },
                scope.clone(),
            )
            .await
    {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    if let Some(startup_command) = body.startup_command
        && let Err(e) = state
            .engine
            .apply_wire_scoped(
                WireCommand::UpdateProjectStartupCommand {
                    project_id: id.clone(),
                    startup_command,
                },
                scope.clone(),
            )
            .await
    {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    if let Some(env) = body.env
        && let Err(e) = state
            .engine
            .apply_wire_scoped(
                WireCommand::UpdateProjectEnv {
                    project_id: id.clone(),
                    env,
                },
                scope,
            )
            .await
    {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    StatusCode::OK.into_response()
}

// ── Reorder ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ReorderBody {
    project_ids: Vec<String>,
}

async fn reorder_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderBody>,
) -> Response {
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::ReorderProjects {
                project_ids: body.project_ids,
            },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Pull / checkout-default ──────────────────────────────────────────────────

async fn pull_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_project();
    }
    if !project_exists(&state, &id).await {
        return unknown_project();
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::PullProject { project_id: id },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn checkout_default(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_project();
    }
    if !project_exists(&state, &id).await {
        return unknown_project();
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::CheckoutProjectDefaultBranch { project_id: id },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn project_exists(state: &AppState, id: &str) -> bool {
    state
        .engine
        .spine()
        .await
        .map(|spine| spine.projects.iter().any(|p| p.id == id))
        .unwrap_or(false)
}

fn unknown_project() -> Response {
    (StatusCode::NOT_FOUND, "unknown project").into_response()
}

fn engine_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "the engine is unavailable; retry shortly",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use std::path::Path;
    use tower::ServiceExt;

    use crate::test_support::router_no_auth;

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

    fn post_add(path: &str, create_initial_commit: bool) -> Request<Body> {
        let body = format!(
            r#"{{"path":{},"create_initial_commit":{}}}"#,
            serde_json::to_string(path).unwrap(),
            create_initial_commit
        );
        Request::builder()
            .method("POST")
            .uri("/api/v1/projects")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn add_with_create_initial_commit_flag_births_head_then_registers() {
        let repo = tempfile::tempdir().unwrap();
        init_repo_no_commit(repo.path());
        let path = repo.path().to_string_lossy().to_string();
        assert!(!dux_core::git::repo_has_commits(repo.path()));

        let (_tmp, app) = router_no_auth();
        let resp = app.oneshot(post_add(&path, true)).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::CREATED,
            "add should succeed and create the project"
        );
        assert!(
            dux_core::git::repo_has_commits(repo.path()),
            "the repo must have a commit after adding with create_initial_commit=true"
        );
    }

    #[tokio::test]
    async fn plain_add_of_unborn_repo_is_rejected_without_committing() {
        // Fail closed: a plain add (no create_initial_commit flag) of a
        // commit-less repo must be rejected by the engine, not silently
        // registered, and must not fabricate a commit. Clients birth the repo
        // via the create_initial_commit flag instead.
        let repo = tempfile::tempdir().unwrap();
        init_repo_no_commit(repo.path());
        let path = repo.path().to_string_lossy().to_string();

        let (_tmp, app) = router_no_auth();
        let resp = app.oneshot(post_add(&path, false)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(
            !dux_core::git::repo_has_commits(repo.path()),
            "a rejected plain add must not create a commit"
        );
    }

    fn post_add_init_repo(path: &str) -> Request<Body> {
        let body = format!(
            r#"{{"path":{},"init_repo":true}}"#,
            serde_json::to_string(path).unwrap()
        );
        Request::builder()
            .method("POST")
            .uri("/api/v1/projects")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn add_with_init_repo_flag_rejects_subdirs_and_existing_repos() {
        // Catches validation bypass at the HTTP boundary: `init_repo: true`
        // must be refused for a repo subdirectory and for an existing repo
        // root, with the engine's validation messages surfacing as 400s.
        let repo = tempfile::tempdir().unwrap();
        init_repo_no_commit(repo.path());
        let sub = repo.path().join("src");
        std::fs::create_dir(&sub).unwrap();

        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(post_add_init_repo(&sub.to_string_lossy()))
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(
            !sub.join(".git").exists(),
            "no nested repository may have been created"
        );

        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(post_add_init_repo(&repo.path().to_string_lossy()))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "an existing repo root must not be re-initialized"
        );
    }

    /// Init a repo with `git init` and one commit.
    fn init_repo_with_commit(dir: &Path) {
        init_repo_no_commit(dir);
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
        run(&["commit", "--allow-empty", "-q", "-m", "init"]);
    }

    fn commit_count(dir: &Path) -> String {
        let out = std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn create_initial_commit_flag_on_already_born_repo_registers_without_a_second_commit() {
        // Race: a commit landed between the client's inspect and this request.
        // The flag must gracefully register the repo (no error, no extra commit),
        // not hard-fail — it's a bootstrap no-op when there's nothing to bootstrap.
        let repo = tempfile::tempdir().unwrap();
        init_repo_with_commit(repo.path());
        let before = commit_count(repo.path());
        let path = repo.path().to_string_lossy().to_string();

        let (_tmp, app) = router_no_auth();
        let resp = app.oneshot(post_add(&path, true)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
        assert_eq!(
            commit_count(repo.path()),
            before,
            "a born repo must not gain a second commit"
        );
    }

    /// Add a committed repo as a project through the same router and return its id.
    async fn add_project_and_id(app: &axum::Router, path: &str) -> String {
        let resp = app.clone().oneshot(post_add(path, false)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        json["id"].as_str().expect("created id").to_string()
    }

    fn delete_project_req(id: &str, delete_worktrees: bool) -> Request<Body> {
        let uri = if delete_worktrees {
            format!("/api/v1/projects/{id}?delete_worktrees=true")
        } else {
            format!("/api/v1/projects/{id}")
        };
        Request::builder()
            .method("DELETE")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn router_with_launching_project_session() -> (tempfile::TempDir, axum::Router) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = dux_core::config::DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        std::fs::write(
            &paths.config_path,
            format!(
                "[[projects]]\nid = \"p1\"\npath = \"{}\"\nname = \"Project\"\n",
                tmp.path().to_string_lossy()
            ),
        )
        .unwrap();
        let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
        let now = chrono::Utc::now();
        store
            .upsert_session(&dux_core::model::AgentSession {
                id: "s1".to_string(),
                provider: dux_core::model::ProviderKind::new("claude"),
                title: None,
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: dux_core::model::SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
                workspace: dux_core::model::AgentWorkspace::Managed(
                    dux_core::model::ManagedWorkspace {
                        project_id: "p1".to_string(),
                        project_path: None,
                        source_branch: "main".to_string(),
                        branch_name: "feature".to_string(),
                        initial_branch: "feature".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_path: tmp.path().to_string_lossy().to_string(),
                    },
                ),
            })
            .unwrap();
        drop(store);
        let mut engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        engine.mark_in_flight(dux_core::engine::InFlightKey::AgentLaunch("s1".to_string()));
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        (tmp, crate::server::router(handle))
    }

    #[tokio::test]
    async fn project_delete_reports_a_launch_refusal_as_conflict() {
        let (_tmp, app) = router_with_launching_project_session();

        let response = app.oneshot(delete_project_req("p1", true)).await.unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("still launching"));
    }

    #[tokio::test]
    async fn delete_project_with_worktrees_flag_removes_the_project_and_keeps_the_source_checkout()
    {
        // The `?delete_worktrees=true` branch routes to `DeleteProject`, which
        // removes the agents' worktrees but must NEVER touch the source checkout.
        // With no agents there is nothing to remove, so this asserts the route is
        // wired (204, project gone) and the safety property (source dir intact).
        let repo = tempfile::tempdir().unwrap();
        init_repo_with_commit(repo.path());
        let path = repo.path().to_string_lossy().to_string();

        let (_tmp, app) = router_no_auth();
        let id = add_project_and_id(&app, &path).await;

        let resp = app
            .clone()
            .oneshot(delete_project_req(&id, true))
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NO_CONTENT);
        assert!(
            repo.path().join(".git").exists(),
            "the source checkout must survive a project delete"
        );
    }

    #[tokio::test]
    async fn plain_delete_removes_the_project_and_keeps_worktrees() {
        // The default DELETE (no flag) routes to `RemoveProject` and returns 204.
        let repo = tempfile::tempdir().unwrap();
        init_repo_with_commit(repo.path());
        let path = repo.path().to_string_lossy().to_string();

        let (_tmp, app) = router_no_auth();
        let id = add_project_and_id(&app, &path).await;

        let resp = app
            .clone()
            .oneshot(delete_project_req(&id, false))
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NO_CONTENT);
        assert!(repo.path().join(".git").exists());
    }

    #[tokio::test]
    async fn create_initial_commit_works_on_a_bare_repo_over_rest() {
        let repo = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run(&["init", "--bare", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "Test"]);
        let path = repo.path().to_string_lossy().to_string();

        let (_tmp, app) = router_no_auth();
        let resp = app.oneshot(post_add(&path, true)).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
        assert!(dux_core::git::repo_has_commits(repo.path()));
    }
}
