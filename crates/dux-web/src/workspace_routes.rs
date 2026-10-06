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
//! - `GET /api/v1/terminals` is the thin read of every terminal, flat and
//!   owner-tagged, standalone ones included.
//! - The thin reads answer from the engine as it is at the request, while the
//!   workspace document is the last one pushed, rebuilt on the engine's next
//!   spine check; a client reading right after its own change reads a thin one.
//! - `GET /api/v1/sessions` also lists, after the live agents, each agent a
//!   followed delete is still removing, as it looked when the delete started,
//!   with `"removing": true` and no terminals, until the delete's operation
//!   record finishes. The workspace document never does: the agent left it when
//!   the delete started.
//! - `GET /api/v1/sessions` (its live rows) and `GET /api/v1/sessions/:id`
//!   carry `remote_viewers`: how many browser attachments the agent has, from
//!   the attachment registry. The workspace document leaves it out, because it
//!   is pushed on every change and a count that moves with every attach would
//!   push the whole document each time.
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
    /// Set only on `GET /api/v1/sessions`, for an agent whose delete is still
    /// running: it has left the workspace, and stays listed, marked, until the
    /// delete's operation record finishes. Absent on every other row.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    removing: bool,
    /// How many browser attachments the agent has across its tabs and its own
    /// terminals, leaving out the terminal UI: set on the live rows of
    /// `GET /api/v1/sessions` and on `GET /api/v1/sessions/:id`, absent on
    /// every other answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_viewers: Option<usize>,
}

impl SessionWithTerminals {
    pub fn new(session: SessionView, terminals: Vec<TerminalView>) -> Self {
        Self {
            session,
            terminals,
            removing: false,
            remote_viewers: None,
        }
    }

    /// This row with its count of remote viewers, read from the attachment
    /// registry, the one record of who is attached to what.
    fn counting_viewers(mut self, attachments: &dux_core::attachments::Attachments) -> Self {
        let scope = dux_core::attachments::Scope {
            agents: [self.session.id.clone()].into(),
            ..Default::default()
        };
        self.remote_viewers = Some(
            attachments
                .blockers(
                    &scope,
                    Some(dux_core::attachments::TERMINAL_UI_CONNECTION),
                    std::time::Instant::now(),
                )
                .len(),
        );
        self
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
        .route("/api/v1/terminals", get(get_terminals))
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

/// Every terminal, flat and owner-tagged, in manual order: the one thin read
/// that lists standalone terminals, which own nothing to nest under.
async fn get_terminals(State(state): State<AppState>) -> Response {
    match state.engine.spine().await {
        Some(spine) => Json(spine.terminals).into_response(),
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
                        .counting_viewers(state.engine.attachments())
                })
                .collect();
            // The agents a delete is still removing, after the live ones. An
            // agent that is somehow still live is listed once, as live.
            let removing: Vec<SessionWithTerminals> = state
                .engine
                .operations()
                .removing_agents()
                .into_iter()
                .filter(|gone| !sessions.iter().any(|live| live.session.id == gone.id))
                .map(|session| SessionWithTerminals {
                    session,
                    terminals: Vec::new(),
                    removing: true,
                    remote_viewers: None,
                })
                .collect();
            let mut sessions = sessions;
            sessions.extend(removing);
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
        Some(Some((session, terminals))) => Json(
            SessionWithTerminals::new(session, terminals)
                .counting_viewers(state.engine.attachments()),
        )
        .into_response(),
        Some(None) => (StatusCode::NOT_FOUND, "unknown session").into_response(),
        None => engine_unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use axum::body::Body;
    use axum::http::Request;
    use dux_core::operations::OperationKind;
    use dux_core::statusline::StatusTone;
    use tower::ServiceExt;

    use super::*;

    async fn get_json(app: &Router, uri: &str) -> serde_json::Value {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// An agent whose delete is still removing its worktree has already left
    /// the workspace. The thin list a script reads still names it, marked as
    /// being removed, until the delete's record finishes; the document the
    /// browser reads does not. Every live row counts the browsers attached to
    /// the agent, leaving out the terminal UI, on the thin reads.
    #[tokio::test]
    async fn the_thin_read_lists_agents_being_removed_and_counts_remote_viewers() {
        use dux_core::attachments::{ConnectionFacts, Heard, Surface, Target, TargetKind};

        let tmp = dux_core::test_scratch::ScratchDir::new();
        let mut engine = crate::test_support::unstarted_test_engine(tmp.path());
        engine
            .sessions
            .push(crate::test_support::sample_agent("gone"));
        let view = engine.session_view("gone").expect("a view of the agent");
        engine.sessions.clear();
        engine.open_operation("op-d", OperationKind::AgentDelete);
        engine.operations.set_removing("op-d", view);
        engine
            .sessions
            .push(crate::test_support::sample_agent("live"));
        let slot = Target {
            kind: TargetKind::Tab,
            id: "live-slot".to_string(),
            agent: Some("live".to_string()),
        };
        for (connection, surface) in [
            ("e1", Surface::Browser),
            ("e2", Surface::Browser),
            (
                dux_core::attachments::TERMINAL_UI_CONNECTION,
                Surface::TerminalUi,
            ),
        ] {
            engine.attachments.register(
                connection,
                ConnectionFacts {
                    surface,
                    device: None,
                    address: None,
                    verified: true,
                    events: true,
                },
                Some(Heard::now()),
            );
            engine
                .attachments
                .attach(connection, slot.clone(), Some(Heard::now()), None)
                .unwrap();
        }
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        let app = crate::server::router(handle.clone());

        let listed = get_json(&app, "/api/v1/sessions").await;
        let rows = listed.as_array().expect("an array");
        assert_eq!(rows.len(), 2, "{listed}");
        assert_eq!(rows[0]["id"], "live");
        assert_eq!(rows[0]["remote_viewers"], 2, "{listed}");
        assert_eq!(rows[1]["id"], "gone");
        assert_eq!(rows[1]["removing"], true);
        assert_eq!(rows[1]["terminals"], serde_json::json!([]));
        let shown = get_json(&app, "/api/v1/sessions/live").await;
        assert_eq!(shown["remote_viewers"], 2, "{shown}");

        let document = get_json(&app, "/api/v1/workspace").await;
        let ids: Vec<&str> = document["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["live"], "{document}");

        handle
            .operations()
            .finish("op-d", StatusTone::Info, "Deleted.", None, Instant::now());
        let listed = get_json(&app, "/api/v1/sessions").await;
        assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    }
}
