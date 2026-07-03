//! REST verbs for agent tabs. Live tab byte I/O rides the nested PTY socket
//! `/ws/sessions/:id/tabs/:tab/pty` (see `server.rs`); these routes manage only
//! tab lifecycle and provider. All tabs are generic; closing any one tab that is
//! the agent's LAST live tab detaches the agent. (The distinct agent-level
//! "Detach agent" action, which stops every tab at once, is `POST .../kill`.)
//!
//! Routes (all gated; an unauthenticated request 401s before the handler):
//! - `POST   /api/v1/sessions/:id/tabs`            — create a tab running
//!   `{ "provider"? }` (the session's project default when omitted). 201 +
//!   `{ "tab_id", "provider" }`. 404 when `:id` is unknown; 400 when the provider
//!   is not configured.
//! - `DELETE /api/v1/sessions/:id/tabs/:tab`       — close one tab. For the
//!   session-slot tab (`:tab == :id`, which has no row) this stops that tab via
//!   `KillSessionPty`, detaching the agent only if it was the last live tab, and
//!   returns 200 + `{ "detached": <bool> }`. Any other tab is closed and its row
//!   removed, 204. A `:tab` not owned by `:id` is a 404.
//! - `PATCH  /api/v1/sessions/:id/tabs/:tab`       — retarget the tab's provider
//!   `{ "provider" }`. 200 on success; 400 when the provider is not configured.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, patch, post},
};
use serde::{Deserialize, Serialize};

use dux_core::wire::WireCommand;

use crate::git_routes::resolve_worktree;
use crate::rest_common::{id_within_bound, scope_from_headers, unknown_session};
use crate::server::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions/{id}/tabs", post(create_tab))
        .route("/api/v1/sessions/{id}/tabs/{tab}", delete(delete_tab))
        .route("/api/v1/sessions/{id}/tabs/{tab}", patch(retarget_tab))
}

#[derive(Deserialize, Default)]
struct CreateTabBody {
    #[serde(default)]
    provider: Option<String>,
}

/// 201 body for a tab create: the new tab id (used to open the nested PTY socket)
/// plus the effective provider it launched with.
#[derive(Serialize)]
struct CreatedTab {
    tab_id: String,
    provider: String,
}

/// 200 body for a Main-tab detach, distinguishing it from a Support-tab close
/// (which is a bodiless 204).
#[derive(Serialize)]
struct DetachedTab {
    detached: bool,
}

#[derive(Deserialize)]
struct RetargetBody {
    provider: String,
}

/// `POST /api/v1/sessions/:id/tabs` — create a Support tab. Direct-return through
/// the dedicated engine request (mints the id synchronously; the launch is async),
/// mirroring `create_terminal`.
async fn create_tab(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<CreateTabBody>>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    let provider = body.and_then(|b| b.0.provider);
    match state.engine.create_agent_tab(id.clone(), provider).await {
        Ok((tab_id, provider)) => {
            let location = format!("/api/v1/sessions/{id}/tabs/{tab_id}");
            (
                StatusCode::CREATED,
                [(header::LOCATION, location)],
                Json(CreatedTab { tab_id, provider }),
            )
                .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// `DELETE /api/v1/sessions/:id/tabs/:tab` — close one tab. The session-slot tab
/// (`tab == id`) stops via `KillSessionPty` (detaches the agent only if it was the
/// last live tab); any other tab is closed. A `:tab` not owned by `:id` is a 404.
async fn delete_tab(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) || !id_within_bound(&tab) {
        return unknown_tab();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    // The session-slot tab has no row, so its "close" goes through the single-tab
    // KillSessionPty path: it stops that tab and detaches the agent only if it was
    // the last live one (any other tabs keep running).
    if tab == id {
        return match state
            .engine
            .apply_wire_scoped(
                WireCommand::KillSessionPty { session_id: id.clone() },
                scope_from_headers(&headers, &state.connections),
            )
            .await
        {
            // `KillSessionPty` only detaches the agent when it was the LAST
            // live tab; with live siblings the agent stays Active. Read the
            // session's real post-kill status rather than hardcoding true, so
            // the client's `{ "detached": <bool> }` reflects what actually
            // happened.
            Ok(_) => {
                let detached = state
                    .engine
                    .session(id)
                    .await
                    .flatten()
                    .map(|s| s.status == "detached")
                    .unwrap_or(false);
                (StatusCode::OK, Json(DetachedTab { detached })).into_response()
            }
            Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
        };
    }
    // Support tab: enforce ownership (never a cross-session close), then close it.
    match state.engine.tab_session(tab.clone()).await {
        Some(owner) if owner == id => {}
        _ => return unknown_tab(),
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::CloseAgentTab {
                session_id: id,
                tab_id: tab,
            },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        // A concurrent close removed the row between the ownership check and the
        // command: "gone" is 404, not a validation error (mirrors kill_session).
        Err(e) if e.contains("unknown tab") => (StatusCode::NOT_FOUND, e).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// `PATCH /api/v1/sessions/:id/tabs/:tab` — retarget the tab's provider (effective
/// on its next launch). `tab == id` retargets the Main tab (delegates to the
/// session-level change); a Support `:tab` must belong to `:id`.
async fn retarget_tab(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<RetargetBody>,
) -> Response {
    if !id_within_bound(&id) || !id_within_bound(&tab) {
        return unknown_tab();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    // Support tabs must belong to the path session; the Main tab (tab == id) is
    // always valid and delegates to the session-level provider change.
    if tab != id {
        match state.engine.tab_session(tab.clone()).await {
            Some(owner) if owner == id => {}
            _ => return unknown_tab(),
        }
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::ChangeAgentTabProvider {
                session_id: id,
                tab_id: tab,
                provider: body.provider,
            },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        // A concurrent close removed the row between the ownership check and the
        // command: "gone" is 404, not a validation error (mirrors kill_session).
        Err(e) if e.contains("unknown tab") => (StatusCode::NOT_FOUND, e).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

fn unknown_tab() -> Response {
    (StatusCode::NOT_FOUND, "unknown tab").into_response()
}
