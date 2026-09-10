//! REST verbs for agent tabs. Live tab byte I/O rides the nested PTY socket
//! `/ws/sessions/:id/tabs/:tab/pty` (see `server.rs`); these routes manage only
//! tab lifecycle and provider. All tabs are generic; closing any one tab that is
//! the agent's LAST live tab detaches the agent. (The distinct agent-level
//! "Detach agent" action, which stops every tab at once, is `POST .../kill`.)
//!
//! Every tab of `:id` is addressable at `.../tabs/:tab`, the session-slot tab
//! included, and so is its PTY socket. What each verb does with the slot tab is a
//! stated decision per route rather than a consequence of the slot tab having no
//! `agent_tabs` row: `DELETE` closes it by PROMOTING the next tab in strip order
//! into the slot, which is a pointer move, so the successor keeps its own id, row,
//! process and sockets and only changes role; `PATCH` accepts it and delegates to
//! the session-level provider change. Neither infers slot-ness from a missing row;
//! both ask `EngineHandle::is_slot_tab`.
//!
//! Closing an agent's ONLY tab is refused with a 400: an agent always has a slot,
//! and that gesture is the agent's detach. A `:tab` not owned by `:id` is a 404,
//! and a provider that is not configured is a 400.
//!
//! `POST .../tabs/:tab/start` is the only start that gets past a recorded launch
//! failure: opening a failed tab's PTY socket deliberately refuses to launch it, so
//! a tab that cannot come up never relaunches itself.
//!
//! `PUT /api/v1/sessions/:id/focused-tab` normalizes a `tab_id` equal to `:id`, or
//! one naming anything but a live extra tab of `:id`, to "no memory", which resolves
//! to the session-slot tab, rather than rejecting it.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, patch, post, put},
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
        .route("/api/v1/sessions/{id}/tabs/{tab}/start", post(start_tab))
        .route(
            "/api/v1/sessions/{id}/focused-tab",
            put(set_focused_tab_route),
        )
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

/// 200 body for a tab close: whether the close detached the agent, plus the tab
/// that took the session slot when the closed tab was the one holding it.
#[derive(Serialize)]
struct ClosedTab {
    detached: bool,
    /// Absent for an ordinary extra tab's close, so the client can tell "nothing
    /// was promoted" from "some tab was" without a sentinel.
    #[serde(skip_serializing_if = "Option::is_none")]
    promoted: Option<String>,
}

#[derive(Deserialize)]
struct RetargetBody {
    provider: String,
}

#[derive(Deserialize)]
struct SetFocusedTabBody {
    tab_id: Option<String>,
}

/// `POST /api/v1/sessions/:id/tabs` - create an extra tab. Direct-return through
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
        return resp.into_response();
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

/// Resolve a `.../tabs/:tab` address: `:id` names a live session and `:tab` is one
/// of its tabs. Every `:tab`-scoped verb opens with this, so they cannot drift on
/// what a bad address answers. The refusal is boxed because an axum response is wide
/// enough to trip clippy's large-error lint on macOS.
///
/// The two ids are checked separately: an out-of-bound `:id` is an unknown SESSION,
/// and collapsing both into `unknown_tab()` blames the tab for a bad session id.
/// Slot-ness decides only whether the ownership check applies, because the slot tab
/// is named by the session's pointer rather than the extra-tab map `tab_session`
/// answers from; an extra tab must belong to `:id`, so no verb reaches across
/// sessions.
async fn resolve_tab_of_session(
    state: &AppState,
    id: &str,
    tab: &str,
) -> Result<(), Box<Response>> {
    if !id_within_bound(id) {
        return Err(Box::new(unknown_session()));
    }
    if !id_within_bound(tab) {
        return Err(Box::new(unknown_tab()));
    }
    if let Err(resp) = resolve_worktree(state, id.to_string()).await {
        return Err(Box::new(resp.into_response()));
    }
    if !state.engine.is_slot_tab(id.to_string(), tab).await {
        match state.engine.tab_session(tab.to_string()).await {
            Some(owner) if owner == id => {}
            _ => return Err(Box::new(unknown_tab())),
        }
    }
    Ok(())
}

/// `DELETE /api/v1/sessions/:id/tabs/:tab` - close one tab. Naming the
/// session-slot tab promotes the next tab in strip order into it; the agent's
/// only tab is refused by the engine, because an agent always has a slot. A
/// `:tab` not owned by `:id` is a 404.
async fn delete_tab(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = resolve_tab_of_session(&state, &id, &tab).await {
        return *resp;
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::CloseAgentTab {
                session_id: id.clone(),
                tab_id: tab,
            },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        // `Engine::close_tab` detaches the agent the same way `KillSessionPty`
        // does when this was the session's LAST live tab, and returns that
        // in-flight-aware outcome on the wire result. Consume it directly rather
        // than re-deriving from `has_live_process`, which cannot see a launch
        // still in flight for a sibling tab.
        Ok(outcome) => {
            let detached = outcome.detached.unwrap_or(true);
            (
                StatusCode::OK,
                Json(ClosedTab {
                    detached,
                    promoted: outcome.promoted,
                }),
            )
                .into_response()
        }
        // A concurrent close removed the row between the ownership check and the
        // command: "gone" is 404, not a validation error (mirrors kill_session).
        Err(e) if e.contains("unknown tab") => (StatusCode::NOT_FOUND, e).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// `POST /api/v1/sessions/:id/tabs/:tab/start` - start a dormant tab. The
/// session-slot tab is as valid a target as any extra tab; slot-ness is asked of
/// the resolver, never inferred from a missing row.
async fn start_tab(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
) -> Response {
    if let Err(resp) = resolve_tab_of_session(&state, &id, &tab).await {
        return *resp;
    }
    match state.engine.start_agent_tab(tab).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// `PATCH /api/v1/sessions/:id/tabs/:tab` - retarget the tab's provider (effective
/// on its next launch). Naming the session-slot tab retargets it (delegates to
/// the session-level change); an extra `:tab` must belong to `:id`.
async fn retarget_tab(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<RetargetBody>,
) -> Response {
    if let Err(resp) = resolve_tab_of_session(&state, &id, &tab).await {
        return *resp;
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

/// `PUT /api/v1/sessions/:id/focused-tab`: remember the tab the user last focused on
/// this agent. Silent, since `SetLastFocusedTab` carries no status, and the engine
/// normalizes an id equal to `:id`, or a tab not owned by `:id`, down to "no memory"
/// rather than erroring, so this handler only validates that the session exists.
async fn set_focused_tab_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SetFocusedTabBody>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp.into_response();
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::SetLastFocusedTab {
                session_id: id,
                tab_id: body.tab_id,
            },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

fn unknown_tab() -> Response {
    (StatusCode::NOT_FOUND, "unknown tab").into_response()
}
