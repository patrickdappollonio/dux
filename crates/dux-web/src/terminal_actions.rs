//! REST verbs for companion terminals: create and delete a terminal for
//! either owner. Live terminal byte I/O rides
//! the nested PTY sockets `/ws/sessions/:id/terminals/:tid/pty` and
//! `/ws/projects/:id/terminals/:tid/pty` (see `server.rs`); these routes manage
//! only the terminal's lifecycle.
//!
//! A session terminal nests under its session, a project terminal under its
//! project, and a standalone terminal has no owner to nest under, so it lives at
//! the un-nested `/api/v1/terminals` and takes no path parameter: nothing has to
//! exist first. Each address serves its own kind only, so a `:tid` belonging to a
//! different owner, or to none, is a 404 rather than a way past ownership.
//!
//! Ownership of `:tid` against the path owner is enforced before every delete,
//! because `DeleteTerminal` looks a terminal up by id alone and checks nothing.
//!
//! `POST /api/v1/terminals/reorder` reorders every companion terminal, all three
//! owners, as one flat global list, and its body is the complete set of ids in the
//! desired order. Runtime-only, with no persistence.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use serde::{Deserialize, Serialize};

use dux_core::model::TerminalRoute;
use dux_core::wire::WireCommand;

use crate::git_routes::resolve_worktree;
use crate::rest_common::{id_within_bound, scope_from_headers, unknown_session};
use crate::server::AppState;

/// The companion-terminal routes. Session terminals nest under
/// `/sessions/:id` and project terminals under `/projects/:id`, so the owner is
/// resolved/validated from the path, exactly like the other resource-nested REST
/// routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions/{id}/terminals", post(create_terminal))
        .route(
            "/api/v1/sessions/{id}/terminals/{tid}",
            delete(delete_terminal),
        )
        .route(
            "/api/v1/projects/{id}/terminals",
            post(create_project_terminal),
        )
        .route(
            "/api/v1/projects/{id}/terminals/{tid}",
            delete(delete_project_terminal),
        )
        // The literal `reorder` segment is registered BEFORE the `{tid}` delete
        // so a router without literal-over-parameter precedence cannot read a
        // reorder as a terminal id.
        .route("/api/v1/terminals/reorder", post(reorder_terminals))
        .route("/api/v1/terminals", post(create_standalone_terminal))
        .route(
            "/api/v1/terminals/{tid}",
            delete(delete_standalone_terminal),
        )
}

/// 201 body for a terminal create: the new terminal's id (used to open the nested
/// PTY socket) plus its display label.
#[derive(Serialize)]
struct CreatedTerminal {
    terminal_id: String,
    label: String,
}

/// `POST /api/v1/sessions/:id/terminals`: create a companion terminal for a
/// session. Runs through the dedicated engine request; it mints no status, so no
/// `X-Connection-Id` scoping is needed here.
async fn create_terminal(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp.into_response();
    }
    match state.engine.create_terminal(id.clone()).await {
        Ok((terminal_id, label)) => {
            let location = format!("/api/v1/sessions/{id}/terminals/{terminal_id}");
            (
                StatusCode::CREATED,
                [(header::LOCATION, location)],
                Json(CreatedTerminal { terminal_id, label }),
            )
                .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// `POST /api/v1/projects/:id/terminals` creates a project terminal: a plain
/// shell at the project's repo root with no agent attached. Mirrors
/// `create_terminal`, with the project (not a session) resolved from the path.
async fn create_project_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_project();
    }
    if state.engine.project_path(id.clone()).await.is_none() {
        return unknown_project();
    }
    match state.engine.create_project_terminal(id.clone()).await {
        Ok((terminal_id, label)) => {
            let location = format!("/api/v1/projects/{id}/terminals/{terminal_id}");
            (
                StatusCode::CREATED,
                [(header::LOCATION, location)],
                Json(CreatedTerminal { terminal_id, label }),
            )
                .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// `POST /api/v1/terminals` creates a standalone terminal: a plain shell in the
/// user's home directory, owned by nothing. Un-nested and parameterless, because
/// there is no owner to resolve and nothing that has to exist first, which is
/// exactly why the two routes above each begin by resolving theirs and this one
/// does not.
async fn create_standalone_terminal(State(state): State<AppState>) -> Response {
    match state.engine.create_standalone_terminal().await {
        Ok((terminal_id, label)) => {
            let location = format!("/api/v1/terminals/{terminal_id}");
            (
                StatusCode::CREATED,
                [(header::LOCATION, location)],
                Json(CreatedTerminal { terminal_id, label }),
            )
                .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// `DELETE /api/v1/terminals/:tid` deletes a standalone terminal, enforcing
/// through the same exhaustive `is_at_route` that `:tid` really is standalone: an
/// owned terminal is a 404 here, so the un-nested address cannot be used to
/// sidestep the cross-owner rejections on the nested ones.
async fn delete_standalone_terminal(
    State(state): State<AppState>,
    Path(tid): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&tid) {
        return unknown_terminal();
    }
    match state.engine.terminal_owner_of(tid.clone()).await {
        Some(owner) if owner.is_at_route(TerminalRoute::Standalone) => {}
        _ => return unknown_terminal(),
    }
    dispatch_delete(&state, tid, &headers).await
}

/// `DELETE /api/v1/sessions/:id/terminals/:tid`: delete a companion terminal,
/// enforcing that `:tid` is session-owned by `:id` before dispatching the delete.
async fn delete_terminal(
    State(state): State<AppState>,
    Path((id, tid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) || !id_within_bound(&tid) {
        return unknown_terminal();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp.into_response();
    }
    // Route membership is decided by the owner type's exhaustive
    // `is_at_route`: an unknown terminal, one owned by a different session, or a
    // PROJECT terminal (whose id could otherwise collide with a session id) is a
    // 404, never a cross-owner delete.
    match state.engine.terminal_owner_of(tid.clone()).await {
        Some(owner) if owner.is_at_route(TerminalRoute::Session(&id)) => {}
        _ => return unknown_terminal(),
    }
    dispatch_delete(&state, tid, &headers).await
}

/// `DELETE /api/v1/projects/:id/terminals/:tid` deletes a project terminal,
/// enforcing that `:tid` is project-owned by `:id` before dispatching the delete.
async fn delete_project_terminal(
    State(state): State<AppState>,
    Path((id, tid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) || !id_within_bound(&tid) {
        return unknown_terminal();
    }
    if state.engine.project_path(id.clone()).await.is_none() {
        return unknown_project();
    }
    // Route membership through the same exhaustive `is_at_route`: a session-owned
    // terminal is a 404 on the project route, exactly as a project terminal is on
    // the session route.
    match state.engine.terminal_owner_of(tid.clone()).await {
        Some(owner) if owner.is_at_route(TerminalRoute::Project(&id)) => {}
        _ => return unknown_terminal(),
    }
    dispatch_delete(&state, tid, &headers).await
}

/// Body for the global terminal reorder: the complete set of terminal ids in the
/// desired order. Mirrors the sessions `reorder-global` shape.
#[derive(Deserialize)]
struct ReorderTerminalsBody {
    terminal_ids: Vec<String>,
}

/// `POST /api/v1/terminals/reorder`. The flat model's drag for terminals: reorder
/// every companion terminal (all three owners) as one global list.
/// `terminal_ids` must be the complete terminal set; the engine validates strictly
/// and stamps each terminal's runtime `sort_order`. Runtime-only, so no persistence.
async fn reorder_terminals(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderTerminalsBody>,
) -> Response {
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::ReorderTerminals {
                terminal_ids: body.terminal_ids,
            },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// The shared delete dispatch: `WireCommand::DeleteTerminal` is id-keyed and
/// owner-blind by design; ownership was already enforced by the route above.
async fn dispatch_delete(state: &AppState, tid: String, headers: &HeaderMap) -> Response {
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::DeleteTerminal { terminal_id: tid },
            scope_from_headers(headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

fn unknown_terminal() -> Response {
    (StatusCode::NOT_FOUND, "unknown terminal").into_response()
}

fn unknown_project() -> Response {
    (StatusCode::NOT_FOUND, "unknown project").into_response()
}
