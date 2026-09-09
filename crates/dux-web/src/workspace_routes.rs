//! The REST reads for the projects/sessions/sidebar "spine".
//!
//! - `GET /api/v1/workspace` is the whole document, and the one the browser reads.
//!   Terminals ride here as ONE flat collection, each tagged with its owner. A
//!   browser reads it once at boot and is then pushed the same bytes over
//!   `/ws/events` on every change, so N tabs no longer answer one change with N
//!   identical GETs; `rev` is the revision the push carries, so a client can order
//!   what it fetched against what it was pushed.
//! - `GET /api/v1/projects`, `GET /api/v1/sessions` and `GET /api/v1/sessions/:id`
//!   are the thin reads: a documented programmability surface, separate from the
//!   document the browser consumes, and each has always carried a `terminals` array
//!   on the owner. Moving terminals to a flat collection changed what the BROWSER
//!   receives and deliberately not these, so they re-nest each owner's terminals
//!   through [`SessionWithTerminals`] and [`ProjectWithTerminals`].
//!
//! A nested terminal entry carries a tagged `owner` field. That is additive and it
//! is kept, not hidden behind a parallel stripped-down type: the tag says out loud
//! what the nesting only implied. It is also pinned. `tests/ws_transport.rs` asserts
//! the exact key set of a terminal entry on every response that can carry one, the
//! thin reads and the idempotent 200 replay; the session-create 201 is not in that
//! list, because a session just created owns no terminals, and what it pins instead
//! is that the array is present and empty rather than missing.
//!
//! `POST /api/v1/sessions` and its replay also reuse [`SessionWithTerminals`]. The
//! replay always does, so it and a later GET of that session agree field for field;
//! the create's `201` does so only when the session view is available and otherwise
//! answers with a minimal id-only body.
//!
//! Status codes:
//! - 200 with the JSON body.
//! - 404 for an unknown session id on the per-session read.
//! - 503 if the engine actor is gone.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use dux_core::viewmodel::{ProjectView, SessionView, TerminalOwnerView, TerminalView};
use serde::Serialize;

use crate::server::AppState;

/// A `SessionView` with the session's terminals nested under it, which is the
/// shape `GET /api/v1/sessions` and `GET /api/v1/sessions/:id` have always
/// served. `flatten` keeps every session field exactly where it was, so this
/// adds the array back and changes nothing else.
#[derive(Serialize)]
pub struct SessionWithTerminals {
    #[serde(flatten)]
    session: SessionView,
    terminals: Vec<TerminalView>,
}

impl SessionWithTerminals {
    pub fn new(session: SessionView, terminals: Vec<TerminalView>) -> Self {
        Self { session, terminals }
    }
}

/// A `ProjectView` with the project's OWN project terminals nested under it, the
/// shape `GET /api/v1/projects` has always served. A project does not absorb its
/// agents' terminals: those are nested on the agent, exactly as before.
#[derive(Serialize)]
struct ProjectWithTerminals {
    #[serde(flatten)]
    project: ProjectView,
    terminals: Vec<TerminalView>,
}

/// Re-nest the spine's flat, owner-tagged collection under the owners the thin reads
/// document, as `(by session id, by project id)`. The match over the owner is
/// EXHAUSTIVE with no wildcard arm, so a new kind of owner has to be answered for
/// here rather than silently vanishing from these endpoints.
fn nest_terminals_by_owner(
    terminals: Vec<TerminalView>,
) -> (
    std::collections::HashMap<String, Vec<TerminalView>>,
    std::collections::HashMap<String, Vec<TerminalView>>,
) {
    let mut by_session: std::collections::HashMap<String, Vec<TerminalView>> =
        std::collections::HashMap::new();
    let mut by_project: std::collections::HashMap<String, Vec<TerminalView>> =
        std::collections::HashMap::new();
    for terminal in terminals {
        match &terminal.owner {
            TerminalOwnerView::Session { session_id } => by_session
                .entry(session_id.clone())
                .or_default()
                .push(terminal),
            TerminalOwnerView::Project { project_id } => by_project
                .entry(project_id.clone())
                .or_default()
                .push(terminal),
            // Owned by nothing, so it nests under nothing and is dropped here on
            // purpose: these endpoints answer what one session or project has.
            // `GET /api/v1/workspace` is the only read that claims to be complete.
            TerminalOwnerView::Standalone { .. } => {}
        }
    }
    (by_session, by_project)
}

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

/// The workspace read routes. Literal segments are registered before the
/// parameterized `:id` route regardless of framework ordering guarantees.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/workspace", get(get_workspace))
        .route("/api/v1/projects", get(get_projects))
        .route("/api/v1/sessions", get(get_sessions))
        .route("/api/v1/sessions/{id}", get(get_session))
}

async fn get_workspace(State(state): State<AppState>) -> Response {
    // The engine loop's cached serialization, not a per-request re-projection: it is
    // already a JSON string with its `rev` embedded, so it goes back raw rather than
    // being deserialized to be re-serialized. These are the exact bytes the push
    // frame carries, which is what makes the two orderable.
    match state.engine.spine_json().await {
        Some(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        None => engine_unavailable(),
    }
}

async fn get_projects(State(state): State<AppState>) -> Response {
    match state.engine.spine().await {
        Some(spine) => {
            let (_, mut by_project) = nest_terminals_by_owner(spine.terminals);
            let projects: Vec<ProjectWithTerminals> = spine
                .projects
                .into_iter()
                .map(|project| {
                    let terminals = by_project.remove(&project.id).unwrap_or_default();
                    ProjectWithTerminals { project, terminals }
                })
                .collect();
            Json(projects).into_response()
        }
        None => engine_unavailable(),
    }
}

async fn get_sessions(State(state): State<AppState>) -> Response {
    match state.engine.spine().await {
        Some(spine) => {
            let (mut by_session, _) = nest_terminals_by_owner(spine.terminals);
            let sessions: Vec<SessionWithTerminals> = spine
                .sessions
                .into_iter()
                .map(|session| {
                    let terminals = by_session.remove(&session.id).unwrap_or_default();
                    SessionWithTerminals::new(session, terminals)
                })
                .collect();
            Json(sessions).into_response()
        }
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
        Some(Some((session, terminals))) => {
            Json(SessionWithTerminals::new(session, terminals)).into_response()
        }
        Some(None) => (StatusCode::NOT_FOUND, "unknown session").into_response(),
        None => engine_unavailable(),
    }
}
