//! REST verbs for companion terminals (Phase 5 of the REST-first migration):
//! create and delete a session's companion terminal. Live terminal byte I/O rides
//! the nested PTY socket `/ws/sessions/:id/terminals/:tid/pty` (see `server.rs`);
//! these routes manage only the terminal's lifecycle.
//!
//! Routes (all gated; an unauthenticated request 401s before the handler):
//! - `POST   /api/v1/sessions/:id/terminals`       — create a companion terminal,
//!   returning `{ "terminal_id", "label" }` (201 + `Location`). 404 when `:id` is
//!   not a known session.
//! - `DELETE /api/v1/sessions/:id/terminals/:tid`  — delete a companion terminal.
//!   The `:tid` ownership against `:id` is enforced before the delete (the legacy
//!   `DeleteTerminal` looks a terminal up by id alone and does not check
//!   ownership), so a `:tid` that does not belong to `:id` is a 404.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use serde::Serialize;

use dux_core::wire::WireCommand;

use crate::git_routes::resolve_worktree;
use crate::rest_common::{id_within_bound, scope_from_headers};
use crate::server::AppState;

/// The gated companion-terminal routes. Both are nested under `/sessions/:id` so
/// the session is resolved/validated from the path, exactly like the other
/// resource-nested REST routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions/{id}/terminals", post(create_terminal))
        .route(
            "/api/v1/sessions/{id}/terminals/{tid}",
            delete(delete_terminal),
        )
}

/// 201 body for a terminal create: the new terminal's id (used to open the nested
/// PTY socket) plus its display label.
#[derive(Serialize)]
struct CreatedTerminal {
    terminal_id: String,
    label: String,
}

/// `POST /api/v1/sessions/:id/terminals` — create a companion terminal for a
/// session. Runs through the dedicated engine request; it mints no status, so no
/// `X-Connection-Id` scoping is needed here.
async fn create_terminal(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
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

/// `DELETE /api/v1/sessions/:id/terminals/:tid` — delete a companion terminal,
/// enforcing that `:tid` belongs to `:id` before dispatching the delete.
async fn delete_terminal(
    State(state): State<AppState>,
    Path((id, tid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) || !id_within_bound(&tid) {
        return unknown_terminal();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    // Enforce session ownership of the terminal: an unknown terminal, or one owned
    // by a different session, is a 404 (never a cross-session delete).
    match state.engine.terminal_session(tid.clone()).await {
        Some(owner) if owner == id => {}
        _ => return unknown_terminal(),
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::DeleteTerminal { terminal_id: tid },
            scope_from_headers(&headers),
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

fn unknown_session() -> Response {
    (StatusCode::NOT_FOUND, "unknown session").into_response()
}

fn unknown_terminal() -> Response {
    (StatusCode::NOT_FOUND, "unknown terminal").into_response()
}
