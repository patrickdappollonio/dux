//! REST write verbs for projects (Phase 4 of the REST-first migration). Same
//! pattern as [`crate::session_actions`]: each handler derives a per-connection
//! [`StatusScope`] from the optional `X-Connection-Id` header and dispatches the
//! matching [`WireCommand`] via [`EngineHandle::apply_wire_scoped`]. The legacy
//! `/ws` `Command` path keeps working in parallel during the migration.
//!
//! Routes (all gated):
//! - `POST   /api/v1/projects`                 — add (body `{path, name?,
//!   checkout_default?}`); `Idempotency-Key` honored.
//! - `DELETE /api/v1/projects/:id`             — remove (does not touch the checkout).
//! - `PATCH  /api/v1/projects/:id`             — update settings (provider /
//!   auto_reopen / startup_command / env), tri-state per field.
//! - `POST   /api/v1/projects/reorder`         — persist order (literal segment).
//! - `POST   /api/v1/projects/:id/pull`        — refresh the source checkout.
//! - `POST   /api/v1/projects/:id/checkout-default` — switch the checkout to default.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::{Path, State},
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

/// The gated project-action routes. The literal `/reorder` segment is registered
/// alongside `:id`; axum's matcher prefers static segments over `:id`. (The
/// `GET /api/v1/projects` read lives in `spine_routes`; axum merges the per-path
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

    // Pick the add variant. `create_initial_commit` takes precedence over
    // `checkout_default` (an unborn repo has no default branch to check out).
    // Like the checkout-default flow, the engine validates the path, serializes
    // per repo path, and runs the commit on a worker before registering — so the
    // mutating git work never runs on the async reactor here, and a failure (or
    // a repo that gained commits since inspect, which the handler registers as a
    // plain add) surfaces through the keyed status stream.
    let cmd = if body.create_initial_commit {
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

// ── Remove ───────────────────────────────────────────────────────────────────

async fn remove_project(
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
            WireCommand::RemoveProject { project_id: id },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
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
