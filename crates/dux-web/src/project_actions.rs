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

    let cmd = if body.checkout_default {
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
