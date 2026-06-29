//! REST write verbs for sessions/agents (Phase 4 of the REST-first migration).
//! Each handler reads the optional `X-Connection-Id` header → a per-connection
//! [`StatusScope`] so the operation's toasts reach only the originating client,
//! then dispatches the matching [`WireCommand`] through
//! [`EngineHandle::apply_wire_scoped`]. The connection id is the one `/ws/events`
//! hands the client in its `connected` handshake frame.
//!
//! Routes (all gated; an unauthenticated request 401s before the handler):
//! - `POST   /api/v1/sessions`                     — create (body discriminator:
//!   `new` | `fork` | `from_worktree` | `from_pr`); `Idempotency-Key` honored.
//! - `DELETE /api/v1/sessions/:id`                 — delete (`?delete_worktree=`).
//! - `PATCH  /api/v1/sessions/:id`                 — rename / change provider /
//!   toggle auto-reopen (optional body fields).
//! - `POST   /api/v1/sessions/:id/reconnect`       — relaunch (`{force}`).
//! - `POST   /api/v1/sessions/:id/rerun-startup-command` — re-run the agent's
//!   project startup command in its worktree (keyed Busy → final toast).
//! - `POST   /api/v1/sessions/reorder`             — persist order (literal
//!   segment, registered so it does not collide with `:id`).

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{patch, post},
};
use serde::{Deserialize, Serialize};

use dux_core::wire::{WireCommand, WireCommandOutcome};

use crate::git_routes::resolve_worktree;
use crate::rest_common::{
    CREATE_AWAIT_TIMEOUT, FROM_PR_CREATE_AWAIT_TIMEOUT, await_new_session, await_session_for_op,
    id_within_bound, idempotency_key, provider_is_configured, scope_from_headers,
};
use crate::server::AppState;

/// The gated session-action routes. The literal `/reorder` segment is registered
/// alongside the parameterized `:id` routes; axum's matcher prefers static
/// segments over `:id`, so `POST /api/v1/sessions/reorder` never resolves to the
/// `:id` handlers. (The `GET /api/v1/sessions/:id` read lives in `spine_routes`;
/// axum merges the per-path method routers, so the verbs here coexist with it.)
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions", post(create_session))
        .route("/api/v1/sessions/reorder", post(reorder_sessions))
        .route(
            "/api/v1/sessions/{id}",
            patch(patch_session).delete(delete_session),
        )
        .route("/api/v1/sessions/{id}/reconnect", post(reconnect_session))
        .route(
            "/api/v1/sessions/{id}/rerun-startup-command",
            post(rerun_startup_command),
        )
}

// ── Create ───────────────────────────────────────────────────────────────────

/// Discriminated create request. `kind` selects the variant; each maps onto an
/// existing create [`WireCommand`]. `name` is optional everywhere (empty →
/// auto-generated branch/agent name, except `fork`/`from_worktree` which the
/// engine validates per their own rules).
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CreateSessionBody {
    New {
        project_id: String,
        #[serde(default)]
        name: String,
    },
    Fork {
        session_id: String,
        #[serde(default)]
        name: String,
    },
    FromWorktree {
        project_id: String,
        worktree_path: String,
        #[serde(default)]
        name: String,
    },
    FromPr {
        project_id: String,
        pr: String,
        #[serde(default)]
        name: String,
    },
}

impl CreateSessionBody {
    fn into_wire(self) -> WireCommand {
        match self {
            CreateSessionBody::New { project_id, name } => {
                WireCommand::CreateAgent { project_id, name }
            }
            CreateSessionBody::Fork { session_id, name } => {
                WireCommand::ForkSession { session_id, name }
            }
            CreateSessionBody::FromWorktree {
                project_id,
                worktree_path,
                name,
            } => WireCommand::CreateAgentFromWorktree {
                project_id,
                worktree_path,
                name,
            },
            CreateSessionBody::FromPr {
                project_id,
                pr,
                name,
            } => WireCommand::CreateAgentFromPr {
                project_id,
                pr,
                name,
            },
        }
    }
}

async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(raw): Json<serde_json::Value>,
) -> Response {
    // Parse the discriminated body ourselves so a malformed/unknown shape is a
    // clean 400 (axum's typed `Json` rejection would be a 422).
    let body: CreateSessionBody = match serde_json::from_value(raw) {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid create body: {e}")).into_response();
        }
    };

    // Idempotency replay: if this key already produced a session that still
    // exists, return it without creating another.
    let key = idempotency_key(&headers);
    if let Some(key) = &key
        && let Some(prev_id) = state.idempotency.get(key)
        && let Some(Some(session)) = state.engine.session(prev_id).await
    {
        return (StatusCode::OK, Json(session)).into_response();
    }

    // The from-PR create resolves differently: its create op is minted later
    // (inside the PR-lookup followup), so it has no synchronous `created_op_id` and
    // must fall back to the set-difference await with a longer window (the
    // `gh pr view` network call routinely exceeds the default 20s).
    let is_from_pr = matches!(body, CreateSessionBody::FromPr { .. });

    // Snapshot the existing session ids for the from-PR fallback await. The
    // synchronous variants use the race-free op-id path instead and ignore this.
    let pre: std::collections::HashSet<String> = match state.engine.spine().await {
        Some(spine) => spine.sessions.into_iter().map(|s| s.id).collect(),
        None => return engine_unavailable(),
    };

    // Dispatch. A synchronous guard refusal (unknown project, invalid name,
    // un-adoptable worktree) is an `Err` → 400; the in-flight guard returns an
    // `Ok` error-toned status → 409 (an agent is already being created).
    let outcome = match state
        .engine
        .apply_wire_scoped(body.into_wire(), scope_from_headers(&headers))
        .await
    {
        Ok(outcome) => {
            if outcome_is_error(&outcome) {
                let msg = outcome
                    .status
                    .map(|s| s.message)
                    .unwrap_or_else(|| "create rejected".to_string());
                // DEFER: 409 is acceptable for the in-flight guard refusal. A
                // possible future refinement is 503 + `Retry-After` so a client can
                // back off automatically; the frontend already suppresses this
                // toast and the /ws status surfaces the message, so 409 stands.
                return (StatusCode::CONFLICT, msg).into_response();
            }
            outcome
        }
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    // RACE-FREE PATH: `new`/`fork`/`from_worktree` mint the create op
    // synchronously and surface its id, so we resolve OUR exact session via the
    // engine's op→session map even under concurrent creates.
    if let Some(op_id) = outcome.created_op_id {
        return match await_session_for_op(&state.engine, op_id, CREATE_AWAIT_TIMEOUT).await {
            Some(id) => created_response(&state, id, key).await,
            // Dispatched, but the create did not complete within the window (it may
            // still succeed or fail asynchronously; that rides the status stream).
            None => StatusCode::ACCEPTED.into_response(),
        };
    }

    // No synchronous create op id. On the happy path only the from-PR create
    // reaches here (its op is minted later). Fix: a create that produced NEITHER a
    // create op NOR a status did no async work — treat it as a failure rather than
    // spinning out a misleading 202 that would arm a never-resolving client focus
    // token. A from-PR dispatch always returns a busy status, so it is unaffected.
    if outcome.status.is_none() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "the create was accepted but started no work; nothing to wait for",
        )
            .into_response();
    }

    // FALLBACK PATH (from-PR): wait via the set-difference scan with the longer
    // from-PR window. See `await_new_session` for the residual concurrent-create
    // race this path carries.
    let timeout = if is_from_pr {
        FROM_PR_CREATE_AWAIT_TIMEOUT
    } else {
        CREATE_AWAIT_TIMEOUT
    };
    match await_new_session(&state.engine, &pre, timeout).await {
        Some(id) => created_response(&state, id, key).await,
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Build the `201 Created` response for a resolved new session id: record the
/// idempotency key (so a retry replays this session), set `Location`, and return
/// the full session view when projectable, else the bare id.
async fn created_response(state: &AppState, id: String, key: Option<String>) -> Response {
    if let Some(key) = key {
        state.idempotency.record(key, id.clone());
    }
    let location = format!("/api/v1/sessions/{id}");
    let body = match state.engine.session(id.clone()).await {
        Some(Some(session)) => Json(session).into_response(),
        _ => Json(CreatedRef { id }).into_response(),
    };
    (StatusCode::CREATED, [(header::LOCATION, location)], body).into_response()
}

/// Minimal create response when the full session view is unavailable.
#[derive(Serialize)]
struct CreatedRef {
    id: String,
}

// ── Delete ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DeleteQuery {
    /// Also remove the agent's worktree from disk (mirrors the WS command's
    /// `delete_worktree`). Defaults to false (keep the worktree) so a missing
    /// query parameter never deletes user data.
    #[serde(default)]
    delete_worktree: bool,
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::DeleteSession {
                session_id: id,
                delete_worktree: q.delete_worktree,
            },
            scope_from_headers(&headers),
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Patch (rename / provider / auto-reopen) ──────────────────────────────────

/// Optional per-field session update. Any subset may be present; absent fields are
/// untouched. `title` is title-only (never renames the git branch); an empty title
/// clears the custom name back to the branch name. `provider` change is deferred to
/// the next reconnect.
#[derive(Deserialize)]
struct PatchSessionBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    auto_reopen: Option<bool>,
}

/// 200 body for a session PATCH. `provider_change` is `Some("pending_reconnect")`
/// only when the request asked to change the provider, signaling the caller that
/// the live agent did not switch — it takes effect on the next reconnect.
#[derive(Serialize)]
struct PatchSessionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_change: Option<String>,
}

async fn patch_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PatchSessionBody>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    let scope = scope_from_headers(&headers);

    // Validate a provider change UP FRONT, before dispatching any sub-command, so a
    // bad provider can never partially apply after the rename/auto-reopen already
    // committed (the PATCH dispatches its fields as independent wire sub-commands
    // with no rollback). The engine re-validates authoritatively; this is the
    // blast-radius guard. NOTE: the remaining fields (title, auto_reopen) are still
    // applied as separate sub-commands, so a failure in a later one leaves an
    // earlier one committed. That residual non-atomicity across the independent
    // fields is accepted: there is no engine atomic-batch command, and the provider
    // is the only field cross-validated against config (the realistic failure here).
    if let Some(provider) = body.provider.as_deref() {
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

    if let Some(title) = body.title
        && let Err(e) = state
            .engine
            .apply_wire_scoped(
                WireCommand::RenameSession {
                    session_id: id.clone(),
                    title,
                },
                scope.clone(),
            )
            .await
    {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    if let Some(enabled) = body.auto_reopen
        && let Err(e) = state
            .engine
            .apply_wire_scoped(
                WireCommand::ToggleAgentAutoReopen {
                    session_id: id.clone(),
                    enabled,
                },
                scope.clone(),
            )
            .await
    {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }

    let mut provider_change = None;
    if let Some(provider) = body.provider {
        if let Err(e) = state
            .engine
            .apply_wire_scoped(
                WireCommand::ChangeAgentProvider {
                    session_id: id.clone(),
                    provider,
                },
                scope,
            )
            .await
        {
            return (StatusCode::BAD_REQUEST, e).into_response();
        }
        // A provider change never kills a running agent; it takes effect on the
        // next reconnect. Tell the caller so it does not assume the live switch.
        provider_change = Some("pending_reconnect".to_string());
    }

    (
        StatusCode::OK,
        Json(PatchSessionResponse { provider_change }),
    )
        .into_response()
}

// ── Reconnect ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ReconnectBody {
    /// Force a fresh session (tear down any running provider, no resume args).
    /// Defaults to false (resume the prior conversation when supported).
    #[serde(default)]
    force: bool,
}

async fn reconnect_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<ReconnectBody>>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    let force = body.map(|Json(b)| b.force).unwrap_or(false);
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::ReconnectSession {
                session_id: id,
                force,
            },
            scope_from_headers(&headers),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Rerun startup command ────────────────────────────────────────────────────

/// Re-run the agent's project startup command in that agent's worktree (the web
/// counterpart to the TUI's `rerun-startup-command-on-agent` palette command).
/// The engine resolves the session + project, requires a non-empty project
/// startup command, and runs it off-thread; the keyed Busy → final status pair
/// rides the `/ws/events` toast stream back to the initiating client. A missing
/// session/project or absent startup command is the engine's `Err` → 400.
async fn rerun_startup_command(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::RerunStartupCommand { session_id: id },
            scope_from_headers(&headers),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Reorder ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ReorderBody {
    project_id: String,
    session_ids: Vec<String>,
}

async fn reorder_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderBody>,
) -> Response {
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::ReorderSessions {
                project_id: body.project_id,
                session_ids: body.session_ids,
            },
            scope_from_headers(&headers),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn unknown_session() -> Response {
    (StatusCode::NOT_FOUND, "unknown session").into_response()
}

fn engine_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "the engine is unavailable; retry shortly",
    )
        .into_response()
}

/// Whether a wire outcome carried an error-toned status (a soft refusal returned
/// as `Ok`, e.g. the create in-flight guard).
fn outcome_is_error(outcome: &WireCommandOutcome) -> bool {
    outcome
        .status
        .as_ref()
        .map(|s| s.tone == "error")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::test_support::router_no_auth;

    #[tokio::test]
    async fn rerun_startup_command_404_for_unknown_session() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/ghost/rerun-startup-command")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    }
}
