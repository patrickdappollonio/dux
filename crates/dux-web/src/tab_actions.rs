//! REST verbs for agent tabs. Live tab byte I/O rides the nested PTY socket
//! `/ws/sessions/:id/tabs/:tab/pty` (see `server.rs`); these routes manage only
//! tab lifecycle and provider. All tabs are generic; closing any one tab that is
//! the agent's LAST live tab detaches the agent. (The distinct agent-level
//! "Detach agent" action, which stops every tab at once, is `POST .../kill`.)
//!
//! Every route is served plainly: dux has NO authentication, so none of these
//! ever 401s. The open access is deliberate (the single-tenant trusted-access
//! model in CLAUDE.md), and the app-wide guards are not authentication: a
//! Host-header allowlist stops a malicious web page rebinding DNS into this
//! server, and the same-origin check stops another site driving these verbs from a
//! visitor's browser, but a client sending no `Origin` (curl, a script) bypasses
//! it by design.
//!
//! Routes:
//! - `POST   /api/v1/sessions/:id/tabs`            — create a tab running
//!   `{ "provider"? }` (the session's project default when omitted). 201 +
//!   `{ "tab_id", "provider" }`. 404 when `:id` is unknown; 400 when the provider
//!   is not configured.
//! - `DELETE /api/v1/sessions/:id/tabs/:tab`       — close one tab. For the
//!   session-slot tab (`:tab == :id`, which has no row) this stops that tab via
//!   `KillSessionPty`; any other tab is closed and its row removed. Either way,
//!   closing an agent's LAST live tab detaches it, so both branches return
//!   200 + `{ "detached": <bool> }` computed after the close. A `:tab` not
//!   owned by `:id` is a 404.
//! - `PATCH  /api/v1/sessions/:id/tabs/:tab`       — retarget the tab's provider
//!   `{ "provider" }`. 200 on success; 400 when the provider is not configured.
//! - `PUT    /api/v1/sessions/:id/focused-tab`     — remember the tab the user
//!   last focused on this agent, so a later sidebar/bare-route navigation to
//!   this agent restores it. `{ "tab_id": string | null }`. A `tab_id` equal to
//!   `:id`, or naming a tab that isn't a live extra tab of `:id`, is normalized
//!   to "no memory" (resolves to the session-slot tab) rather than rejected.
//!   Fire-and-forget on the client side; 200 on success.

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

/// 200 body for a session-slot tab detach, distinguishing it from an extra-tab close
/// (which is a bodiless 204).
#[derive(Serialize)]
struct DetachedTab {
    detached: bool,
}

#[derive(Deserialize)]
struct RetargetBody {
    provider: String,
}

#[derive(Deserialize)]
struct SetFocusedTabBody {
    tab_id: Option<String>,
}

/// `POST /api/v1/sessions/:id/tabs` — create an extra tab. Direct-return through
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

/// `DELETE /api/v1/sessions/:id/tabs/:tab` — close one tab. The session-slot tab
/// (`tab == id`) stops via `KillSessionPty` (detaches the agent only if it was the
/// last live tab); any other tab is closed. A `:tab` not owned by `:id` is a 404.
async fn delete_tab(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    // Check the session id and the tab id separately: an out-of-bound `:id` is an
    // unknown SESSION (matches `resolve_worktree`'s 404 below), not a tab-worded
    // error: collapsing both into `unknown_tab()` blames the tab even when the
    // session id itself is the bad one.
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if !id_within_bound(&tab) {
        return unknown_tab();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp.into_response();
    }
    // The session-slot tab has no row, so its "close" goes through the single-tab
    // KillSessionPty path: it stops that tab and detaches the agent only if it was
    // the last live one (any other tabs keep running).
    if tab == id {
        return match state
            .engine
            .apply_wire_scoped(
                WireCommand::KillSessionPty {
                    session_id: id.clone(),
                },
                scope_from_headers(&headers, &state.connections),
            )
            .await
        {
            // `KillSessionPty` only detaches the agent when it was the LAST live
            // tab. The engine computes that with the IN-FLIGHT-AWARE
            // `any_tab_active` and returns it on the wire outcome; consume it
            // directly rather than re-deriving from `has_live_process` (a
            // `providers`-only check that misses a sibling's in-flight launch,
            // so a close racing a launch reported `detached: true` wrongly).
            Ok(outcome) => {
                let detached = outcome.detached.unwrap_or(true);
                (StatusCode::OK, Json(DetachedTab { detached })).into_response()
            }
            Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
        };
    }
    // extra tab: enforce ownership (never a cross-session close), then close it.
    match state.engine.tab_session(tab.clone()).await {
        Some(owner) if owner == id => {}
        _ => return unknown_tab(),
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
        // in-flight-aware outcome on the wire result. Consume it directly (the
        // session-slot branch above does the same), instead of re-deriving from
        // `has_live_process`.
        Ok(outcome) => {
            let detached = outcome.detached.unwrap_or(true);
            (StatusCode::OK, Json(DetachedTab { detached })).into_response()
        }
        // A concurrent close removed the row between the ownership check and the
        // command: "gone" is 404, not a validation error (mirrors kill_session).
        Err(e) if e.contains("unknown tab") => (StatusCode::NOT_FOUND, e).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// `PATCH /api/v1/sessions/:id/tabs/:tab` — retarget the tab's provider (effective
/// on its next launch). `tab == id` retargets the session-slot tab (delegates to
/// the session-level change); an extra `:tab` must belong to `:id`.
async fn retarget_tab(
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<RetargetBody>,
) -> Response {
    // See `delete_tab` above: check the session id and tab id separately so a
    // bad `:id` is reported as an unknown session, not a tab-worded error.
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if !id_within_bound(&tab) {
        return unknown_tab();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp.into_response();
    }
    // Extra tabs must belong to the path session; the session-slot tab (tab ==
    // id) is always valid and delegates to the session-level provider change.
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

/// `PUT /api/v1/sessions/:id/focused-tab` — remember the tab the user last
/// focused on this agent (J4: a dedicated route rather than piggybacking on an
/// existing verb, matching the one-verb-per-action style above). Silent
/// (`SetLastFocusedTab` carries no status/toast, J3): the engine itself
/// normalizes an id equal to `:id`, or a tab not owned by `:id`, down to "no
/// memory" rather than erroring, so this handler only needs to validate the
/// session exists.
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
