//! The REST reads for the projects/sessions/sidebar "spine" that used to ride
//! inside every per-tick `ViewModel` broadcast (Phase 3 of the REST-first
//! migration).
//!
//! - `GET /api/v1/spine`, the whole spine `{ projects, sessions, terminals,
//!   sidebar }`. Terminals ride here as ONE flat collection, each tagged with its
//!   owner; this is the document the browser reads. Invalidated by the coarse
//!   `projects.changed` / `sessions.changed` events.
//! - `GET /api/v1/projects` — just the `ProjectView[]` (for programmability).
//! - `GET /api/v1/sessions` — just the `SessionView[]`.
//! - `GET /api/v1/sessions/:id` — one `SessionView`, 404 if unknown.
//!
//! The three thin reads are a documented programmability surface, separate from
//! the spine document the browser consumes, and each has always carried a
//! `terminals` array on the owner. Moving terminals to a flat collection was a
//! change to what the BROWSER receives; it is deliberately NOT a change here, so
//! those reads re-nest each owner's terminals ([`SessionWithTerminals`],
//! [`ProjectWithTerminals`]) and nothing reading them loses information.
//!
//! ## The one shape change, stated plainly
//!
//! A nested terminal entry now carries an `owner` field it did not carry before,
//! a tagged `{"kind":"session","session_id":…}` or
//! `{"kind":"project","project_id":…}`. That is ADDITIVE and it is kept: adding a
//! field is the ordinary way an API grows, a consumer that breaks on an unknown
//! field is already fragile, and the tag says out loud what the nesting only
//! implied. It is NOT hidden behind a parallel stripped-down type.
//!
//! What it is not allowed to be is a surprise. `thin_reads_pin_the_exact_terminal_key_set`
//! and `session_create_and_its_replay_pin_the_same_terminal_key_set`
//! (`tests/ws_transport.rs`) assert the EXACT key set of a terminal entry on
//! every response that can carry one: these three reads and the idempotent 200
//! replay. The session-create 201 is deliberately NOT in that list, because a
//! session that has just been created owns no terminals, so it has no entry to
//! assert against; what the 201 pins instead is that the array is present and
//! empty rather than missing. Note the 201 has TWO shapes, this full view and a
//! minimal id-only fallback for when the view is unavailable, so its terminal
//! entry shape follows the replay's only on the full branch, which is the one
//! the test exercises. Add or remove a field and those
//! fail, which is the point: the previous tests checked only ids and lengths, so
//! `owner` appearing was invisible to the suite.
//!
//! `POST /api/v1/sessions` and its idempotent replay also reuse
//! [`SessionWithTerminals`] (see `session_actions.rs`). The replay always does,
//! so it and a later GET of that session agree field for field. The create's
//! `201` does so only when the session view is available, and otherwise answers
//! with a minimal id-only body, so state that agreement for the replay and not
//! for both.
//!
//! Status codes:
//! - 200 with the JSON body.
//! - 404 for an unknown session id on the per-session read.
//! - 503 if the engine actor is gone (the handle round-trip failed).
//!
//! Served like every other API route. dux has NO authentication of any kind, so
//! nothing here ever 401s. That open access is deliberate: the single-tenant
//! trusted-access model documented in CLAUDE.md. The two app-wide guards are a
//! Host-header allowlist, which stops a malicious web page from rebinding DNS
//! into this server, and a same-origin check that applies to MUTATIONS only, so
//! these GETs are not behind it. Neither guard is authentication.

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

/// Re-nest the spine's flat, owner-tagged collection under the owners the thin
/// reads document: `(by session id, by project id)`.
///
/// The match over the owner is EXHAUSTIVE with no wildcard arm, so a new kind of
/// owner has to be answered for here rather than silently vanishing from these
/// endpoints, which is the whole reason the owner is a tagged value.
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

/// The spine read routes. Literal segments are registered before the
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
