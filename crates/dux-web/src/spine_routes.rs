//! The REST reads for the projects/sessions/sidebar "spine" that used to ride
//! inside every per-tick `ViewModel` broadcast (Phase 3 of the REST-first
//! migration).
//!
//! - `GET /api/v1/spine` — the whole spine `{ projects, sessions, sidebar }`, the
//!   exact shapes the ViewModel used to carry. Invalidated by the coarse
//!   `projects.changed` / `sessions.changed` events.
//! - `GET /api/v1/projects` — just the `ProjectView[]` (for programmability).
//! - `GET /api/v1/sessions` — just the `SessionView[]`.
//! - `GET /api/v1/sessions/:id` — one `SessionView`, 404 if unknown.
//!
//! Status codes:
//! - 200 with the JSON body.
//! - 404 for an unknown session id on the per-session read.
//! - 503 if the engine actor is gone (the handle round-trip failed).
//!
//! Merged into the authenticated (gated) sub-router in `server.rs`, so an
//! unauthenticated request 401s before reaching these handlers.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::server::AppState;

/// Upper bound on the `:id` path segment before any lookup (matches the
/// length-bounding convention for path params elsewhere).
const MAX_ID_LEN: usize = 128;

/// The 503 returned when the engine actor is gone, so a dead engine is
/// distinguishable from a real (possibly empty) payload.
fn engine_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "the engine is unavailable; retry shortly",
    )
        .into_response()
}

/// The gated spine read routes. Literal segments are registered before the
/// parameterized `:id` route regardless of framework ordering guarantees.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/spine", get(get_spine))
        .route("/api/v1/projects", get(get_projects))
        .route("/api/v1/sessions", get(get_sessions))
        .route("/api/v1/sessions/{id}", get(get_session))
}

async fn get_spine(State(state): State<AppState>) -> Response {
    // Served from the engine loop's cached serialization (rebuilt only when the
    // spine changes), not re-projected per request. The cache is already a JSON
    // string, so return it raw with the JSON content-type rather than
    // deserializing just to re-`Json`-serialize it.
    match state.engine.spine_json().await {
        Some(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        None => engine_unavailable(),
    }
}

async fn get_projects(State(state): State<AppState>) -> Response {
    match state.engine.spine().await {
        Some(spine) => Json(spine.projects).into_response(),
        None => engine_unavailable(),
    }
}

async fn get_sessions(State(state): State<AppState>) -> Response {
    match state.engine.spine().await {
        Some(spine) => Json(spine.sessions).into_response(),
        None => engine_unavailable(),
    }
}

async fn get_session(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    // Length-bound the id before any lookup. Count characters, not bytes, so a
    // multi-byte id is not rejected early by its UTF-8 length.
    if id.chars().count() > MAX_ID_LEN {
        return (StatusCode::NOT_FOUND, "unknown session").into_response();
    }
    // Project ONLY the requested session, not the whole spine. The outer `None`
    // is a dead engine (503); the inner `None` is an unknown session id (404).
    match state.engine.session(id).await {
        Some(Some(session)) => Json(session).into_response(),
        Some(None) => (StatusCode::NOT_FOUND, "unknown session").into_response(),
        None => engine_unavailable(),
    }
}
