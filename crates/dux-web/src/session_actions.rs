//! REST write verbs for sessions/agents (Phase 4 of the REST-first migration).
//! Each handler reads the optional `X-Connection-Id` header → a per-connection
//! [`StatusScope`] so the operation's toasts reach only the originating client,
//! then dispatches the matching [`WireCommand`] through
//! [`EngineHandle::apply_wire_scoped`]. The connection id is the one `/ws/events`
//! hands the client in its `connected` handshake frame.
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
//! - `PUT    /api/v1/sessions/:id/pull-request`, manually attach (pin) a
//!   PR from a raw typed reference; `202` + `{op_id}` (deferred, the outcome
//!   rides the toast stream and `sessions.changed`).
//! - `DELETE /api/v1/sessions/:id/pull-request`, detach the manual pin so
//!   autodetection resumes (synchronous, `200`).
//! - `POST   /api/v1/pull-requests/resolve`, read a typed pull-request
//!   reference and say which projects are checkouts of the repository it names.
//!   A READ, not a write: it starts nothing and changes nothing, so it answers
//!   the client directly instead of going through a wire command and a toast.
//!   The client then posts the create with the project it settled on.
//!
//! The idempotent `200` replay always serves
//! [`crate::workspace_routes::SessionWithTerminals`], the same nested shape as
//! `GET /api/v1/sessions/:id`, so a replay and a later read of that session agree
//! field for field. The create's `201` serves that shape too WHENEVER THE VIEW IS
//! AVAILABLE, and falls back to a minimal id-only body when it is not, so the
//! agreement holds on that branch and not unconditionally. A nested terminal
//! entry carries a tagged `owner` field, which
//! is additive and documented in `workspace_routes`'s module docs; the exact key set
//! of the replay body, and of the create body on its full branch, is pinned by
//! `session_create_and_its_replay_pin_the_same_terminal_key_set` in
//! `tests/ws_transport.rs`.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{patch, post, put},
};
use serde::{Deserialize, Serialize};

use dux_core::wire::{WireCommand, WireCommandOutcome};

use crate::git_routes::resolve_worktree;
use crate::rest_common::{
    CREATE_AWAIT_TIMEOUT, FROM_PR_CREATE_AWAIT_TIMEOUT, await_new_session, await_session_for_op,
    id_within_bound, idempotency_key, provider_is_configured, scope_from_headers, unknown_session,
};
use crate::server::AppState;

/// The session-action routes. The literal `/reorder` segment is registered
/// alongside the parameterized `:id` routes; axum's matcher prefers static
/// segments over `:id`, so `POST /api/v1/sessions/reorder` never resolves to the
/// `:id` handlers. (The `GET /api/v1/sessions/:id` read lives in `workspace_routes`;
/// axum merges the per-path method routers, so the verbs here coexist with it.)
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions", post(create_session))
        .route(
            "/api/v1/pull-requests/resolve",
            post(resolve_pull_request_reference),
        )
        .route("/api/v1/sessions/reorder", post(reorder_sessions))
        .route("/api/v1/sessions/reorder-global", post(reorder_agents))
        .route(
            "/api/v1/sessions/{id}",
            patch(patch_session).delete(delete_session),
        )
        .route("/api/v1/sessions/{id}/reconnect", post(reconnect_session))
        .route(
            "/api/v1/sessions/{id}/rerun-startup-command",
            post(rerun_startup_command),
        )
        .route("/api/v1/sessions/{id}/kill", post(kill_session))
        .route(
            "/api/v1/sessions/{id}/pull-request",
            put(attach_pull_request).delete(detach_pull_request),
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
        #[serde(default)]
        copy_uncommitted_changes: Option<bool>,
        /// The client's CONFIRMATION that attaching to an existing branch of the
        /// same name is intended. Absent/false makes the server run the branch
        /// preflight and refuse (409) an unconfirmed existing-branch attach, so
        /// the client can show a confirmation and re-POST with this set.
        #[serde(default)]
        use_existing_branch: bool,
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
            CreateSessionBody::New {
                project_id,
                name,
                copy_uncommitted_changes,
                use_existing_branch,
            } => WireCommand::CreateAgent {
                project_id,
                name,
                use_existing_branch,
                copy_uncommitted_changes,
            },
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
        && let Some(Some((session, terminals))) = state.engine.session(prev_id).await
    {
        // The same nested shape the per-session read serves, so a replay and a
        // later GET of the same session agree field for field.
        return (
            StatusCode::OK,
            Json(crate::workspace_routes::SessionWithTerminals::new(
                session, terminals,
            )),
        )
            .into_response();
    }

    // Existing-branch consent (the "no silent attach" tenet): for a `new` create
    // with a user-typed name that the client has NOT confirmed, run the shared
    // core branch preflight. If it names an existing branch, refuse with a
    // confirmable 409 carrying the branch name + location, so the client shows a
    // confirmation and re-POSTs with `use_existing_branch: true` rather than
    // silently adopting that branch's history. (The wire command enforces the
    // same refusal as defense in depth for a client that skips this dialog.)
    if let CreateSessionBody::New {
        project_id,
        name,
        use_existing_branch: false,
        ..
    } = &body
        && !name.trim().is_empty()
        && let Some(dux_core::git::CreateAgentBranchPlan::ExistingBranch { location }) = state
            .engine
            .create_agent_branch_plan(project_id.clone(), name.trim().to_string())
            .await
    {
        let location = match location {
            dux_core::git::BranchLocation::Local => "local",
            dux_core::git::BranchLocation::Remote => "remote",
        };
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "existing_branch": { "name": name.trim(), "location": location }
            })),
        )
            .into_response();
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
        .apply_wire_scoped(
            body.into_wire(),
            scope_from_headers(&headers, &state.connections),
        )
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
        Some(Some((session, terminals))) => Json(
            crate::workspace_routes::SessionWithTerminals::new(session, terminals),
        )
        .into_response(),
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
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        // An `Ok` error-toned status means the delete was REFUSED, not performed
        // (a tab is still launching, or an async delete is already in flight) —
        // report 409 with the message rather than a misleading 204 "deleted".
        Ok(outcome) => {
            if outcome_is_error(&outcome) {
                let msg = outcome
                    .status
                    .map(|s| s.message)
                    .unwrap_or_else(|| "delete refused".to_string());
                return (StatusCode::CONFLICT, msg).into_response();
            }
            StatusCode::NO_CONTENT.into_response()
        }
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
    let scope = scope_from_headers(&headers, &state.connections);

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
            scope_from_headers(&headers, &state.connections),
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
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Kill (force-detach a running agent) ──────────────────────────────────────

/// Detach the agent WHOLE: stop every one of its tabs' provider processes
/// without deleting its session or worktree (the agent-level "Detach agent"
/// action). The engine drops each provider (SIGKILL) and marks the session
/// Detached, so it can be reconnected. This is distinct from closing a single
/// tab. Unknown session → 404; an agent that is not running is a successful
/// no-op. Companion terminals are killed through the existing
/// `DELETE /api/v1/sessions/:id/terminals/:tid`.
///
/// Note: unlike the git-mutation routes, this does NOT call `resolve_worktree` —
/// killing a PTY needs no worktree on disk (a hung agent whose worktree was
/// removed must still be killable). The engine's own unknown-session error is
/// the existence check, mapped to 404 here.
async fn kill_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::DetachAgent { session_id: id },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        // The engine returns "unknown session: …" when the row is gone (e.g. a
        // concurrent delete); surface that as 404, not a generic 400.
        Err(e) if e.contains("unknown session") => (StatusCode::NOT_FOUND, e).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Pull-request attach / detach ─────────────────────────────────────────────

/// Body of `PUT /api/v1/sessions/:id/pull-request`: the raw reference text the
/// user typed (a PR URL, `#123`, `owner/repo#123`, or a bare number).
#[derive(Deserialize)]
struct AttachPullRequestBody {
    pr: String,
}

/// `202 Accepted` body: the keyed status op id spanning resolve and attach, so
/// a client can correlate the eventual final on the toast stream. This is the
/// documented deferred direction (see `rest_common`): the handler must NOT
/// block on the `gh` lookup.
#[derive(Serialize)]
struct AttachPullRequestAccepted {
    op_id: String,
}

/// Manually attach (pin) a pull request to a session. Dispatch only: the
/// engine mints the keyed busy (broadcast by the actor arm) and spawns the gh
/// lookup worker; the final (attached, or the failure) rides the status toast
/// stream, and the pinned badge lands via `sessions.changed`.
async fn attach_pull_request(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(raw): Json<serde_json::Value>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    // Existence first, before even reading the body (the neighbors' ordering):
    // the engine's dispatch checks the gh gate BEFORE its session lookup, so an
    // unknown id would otherwise surface as the gh message (a 400) instead of
    // the truthful 404.
    match state.engine.session(id.clone()).await {
        None => return engine_unavailable(),
        Some(None) => return unknown_session(),
        Some(Some(_)) => {}
    }
    // Parse the body ourselves so a malformed shape is a clean 400 (axum's
    // typed `Json` rejection would be a 422), matching the create handler.
    let body: AttachPullRequestBody = match serde_json::from_value(raw) {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid attach body: {e}")).into_response();
        }
    };
    match state
        .engine
        .attach_pull_request(
            id,
            body.pr,
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(op_id) => (
            StatusCode::ACCEPTED,
            Json(AttachPullRequestAccepted { op_id }),
        )
            .into_response(),
        // Defense in depth for a session deleted between the check above and
        // the dispatch: the engine's own unknown-session error stays a 404.
        Err(e) if e.contains("unknown session") => (StatusCode::NOT_FOUND, e).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

/// Remove a session's manual pull-request attachment so branch-name
/// autodetection resumes. Synchronous; the info status rides the stream like
/// the sibling handlers' outcomes (the `ApplyWire` arm broadcasts it). A
/// session without a pin is a successful no-op with an honest message.
async fn detach_pull_request(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::ClearPullRequestOverride { session_id: id },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        // The engine returns "unknown session: …" when the row is gone; surface
        // that as 404, not a generic 400 (the `kill_session` pattern).
        Err(e) if e.contains("unknown session") => (StatusCode::NOT_FOUND, e).into_response(),
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
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[derive(Deserialize)]
struct ReorderGlobalBody {
    session_ids: Vec<String>,
}

/// `POST /api/v1/sessions/reorder-global`. The flat model's drag: reorder every
/// agent as one global list. `session_ids` must be the complete session set.
async fn reorder_agents(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderGlobalBody>,
) -> Response {
    match state
        .engine
        .apply_wire_scoped(
            WireCommand::ReorderAgents {
                session_ids: body.session_ids,
            },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn engine_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "the engine is unavailable; retry shortly",
    )
        .into_response()
}

/// Whether a wire outcome carried an error-toned status (a soft refusal returned
/// as `Ok`, e.g. the create in-flight guard). Shared with `project_actions` so
/// the project delete's refusal maps to 409 the same way a session delete's does.
pub(crate) fn outcome_is_error(outcome: &WireCommandOutcome) -> bool {
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

    use tempfile::TempDir;

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

    #[tokio::test]
    async fn kill_session_404_for_unknown_session() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/ghost/kill")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    }

    fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?} failed");
    }

    /// Boot a router whose engine has ONE project pointing at a real git repo
    /// that already has a branch named `existing_branch`. The project is declared
    /// in config.toml so the bootstrap reconciliation adopts it into the engine.
    fn router_with_project_and_branch(existing_branch: &str) -> (TempDir, axum::Router, String) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-b", "main"]);
        run_git(&repo, &["config", "user.name", "t"]);
        run_git(&repo, &["config", "user.email", "t@t"]);
        run_git(&repo, &["commit", "--allow-empty", "-m", "init"]);
        run_git(&repo, &["branch", existing_branch]);

        let paths = dux_core::config::DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        std::fs::write(
            &paths.config_path,
            format!(
                "[[projects]]\nid = \"p1\"\npath = \"{}\"\nname = \"Repo\"\n",
                repo.to_string_lossy()
            ),
        )
        .unwrap();
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        (tmp, crate::server::router(handle), "p1".to_string())
    }

    async fn post_create(app: &axum::Router, body: serde_json::Value) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// #10: an unconfirmed create whose name matches an existing branch is
    /// REFUSED with a confirmable 409 carrying the branch info, instead of
    /// silently attaching to that branch's history.
    #[tokio::test]
    async fn create_refuses_unconfirmed_existing_branch_attach() {
        let (_tmp, app, project_id) = router_with_project_and_branch("feature-x");
        let resp = post_create(
            &app,
            serde_json::json!({
                "kind": "new",
                "project_id": project_id,
                "name": "feature-x",
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["existing_branch"]["name"], "feature-x");
        assert_eq!(json["existing_branch"]["location"], "local");
    }

    /// A create for a FRESH name (no matching branch) is not refused by the
    /// preflight (it proceeds to dispatch).
    #[tokio::test]
    async fn create_does_not_refuse_a_fresh_name() {
        let (_tmp, app, project_id) = router_with_project_and_branch("feature-x");
        let resp = post_create(
            &app,
            serde_json::json!({
                "kind": "new",
                "project_id": project_id,
                "name": "brand-new-name",
            }),
        )
        .await;
        assert_ne!(
            resp.status(),
            StatusCode::CONFLICT,
            "a fresh name must not hit the existing-branch refusal"
        );
    }

    /// Boot a router whose engine has ONE session seeded straight into the
    /// SQLite store before bootstrap (the same seam the engine-actor tests
    /// use), so the pull-request routes can be exercised against a session
    /// that exists without creating a worktree or spawning a provider.
    fn router_with_seeded_session(id: &str) -> (TempDir, axum::Router) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = dux_core::config::DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
        let now = chrono::Utc::now();
        store
            .upsert_session(&dux_core::model::AgentSession {
                id: id.to_string(),
                project_id: "p1".to_string(),
                project_path: None,
                provider: dux_core::model::ProviderKind::new("claude"),
                source_branch: "main".to_string(),
                branch_name: "feat".to_string(),
                initial_branch: "feat".to_string(),
                branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                worktree_path: tmp.path().to_string_lossy().to_string(),
                title: Some("seeded".to_string()),
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: dux_core::model::SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .unwrap();
        drop(store);
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        (tmp, crate::server::router(handle))
    }

    async fn send_json(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let builder = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(b) => builder
                .header("content-type", "application/json")
                .body(axum::body::Body::from(b.to_string()))
                .unwrap(),
            None => builder.body(axum::body::Body::empty()).unwrap(),
        };
        app.clone().oneshot(req).await.unwrap()
    }

    /// PUT on an id no session has is the truthful 404, not the gh-gate 400
    /// (the handler checks existence before dispatching).
    #[tokio::test]
    async fn attach_pull_request_404_for_unknown_session() {
        let (_tmp, app) = router_no_auth();
        let resp = send_json(
            &app,
            "PUT",
            "/api/v1/sessions/ghost/pull-request",
            Some(serde_json::json!({ "pr": "#12" })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    }

    /// A body without the `pr` field on a REAL session is a clean 400 (the
    /// manual parse), never a 422 or a dispatch. On a ghost id the existence
    /// check runs first, so even a malformed body gets the truthful 404.
    #[tokio::test]
    async fn attach_pull_request_400_for_malformed_body() {
        let (_tmp, app) = router_with_seeded_session("s1");
        let resp = send_json(
            &app,
            "PUT",
            "/api/v1/sessions/s1/pull-request",
            Some(serde_json::json!({ "reference": "#12" })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();

        let resp = send_json(
            &app,
            "PUT",
            "/api/v1/sessions/ghost/pull-request",
            Some(serde_json::json!({ "reference": "#12" })),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "existence is checked before the body is read"
        );
    }

    /// Boot a router whose engine has the seeded session, its project (at a
    /// plain NON-GitHub directory), and gh reported available, without ever
    /// touching a real gh: the gate fields are preset before the actor spawns,
    /// and the boot probe runs against the stand-in gh script. The attach
    /// worker then resolves the project's remote (none: not even a git repo),
    /// refuses the bare number BEFORE any `gh pr view`, and reports through
    /// the keyed op, so the happy HTTP path is testable with zero gh calls.
    fn router_with_seeded_session_and_gh(id: &str) -> (TempDir, axum::Router) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = dux_core::config::DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let project_dir = tmp.path().join("plain-project");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            &paths.config_path,
            format!(
                "[[projects]]\nid = \"p1\"\npath = \"{}\"\nname = \"Plain\"\n",
                project_dir.to_string_lossy()
            ),
        )
        .unwrap();
        let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
        let now = chrono::Utc::now();
        store
            .upsert_session(&dux_core::model::AgentSession {
                id: id.to_string(),
                project_id: "p1".to_string(),
                project_path: None,
                provider: dux_core::model::ProviderKind::new("claude"),
                source_branch: "main".to_string(),
                branch_name: "feat".to_string(),
                initial_branch: "feat".to_string(),
                branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                worktree_path: project_dir.to_string_lossy().to_string(),
                title: Some("seeded".to_string()),
                started_providers: Vec::new(),
                desired_running: false,
                auto_reopen_enabled: false,
                status: dux_core::model::SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .unwrap();
        drop(store);
        let mut engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        // The gate the dispatch checks, preset so the PUT cannot race the boot
        // probe; the probe itself is pointed at the stand-in gh so enabling
        // the integration never runs a real gh.
        engine.github_integration_enabled = true;
        engine.gh_status = dux_core::model::GhStatus::Available;
        engine.gh_probe.program =
            dux_core::gh::probe_test_support::stand_in_gh_serving(tmp.path(), &["github.com"])
                .into();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        (tmp, crate::server::router(handle))
    }

    /// The happy HTTP path: a PUT on a real session with gh available replies
    /// `202 Accepted` immediately (no blocking on the lookup) with the keyed
    /// status op id in the body, the codebase's documented deferred shape.
    #[tokio::test]
    async fn attach_pull_request_202_carries_the_op_id() {
        let (_tmp, app) = router_with_seeded_session_and_gh("s1");
        let resp = send_json(
            &app,
            "PUT",
            "/api/v1/sessions/s1/pull-request",
            Some(serde_json::json!({ "pr": "#42" })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let op_id = json["op_id"].as_str().expect("op_id in the 202 body");
        assert!(op_id.starts_with("op-"), "got {op_id:?}");
    }

    /// With a real session but no usable gh (the test engine never probes gh,
    /// so its status stays Unknown), the dispatch refuses synchronously with a
    /// message naming gh, mapped to 400.
    #[tokio::test]
    async fn attach_pull_request_400_mentions_gh_when_unavailable() {
        let (_tmp, app) = router_with_seeded_session("s1");
        let resp = send_json(
            &app,
            "PUT",
            "/api/v1/sessions/s1/pull-request",
            Some(serde_json::json!({ "pr": "#12" })),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let msg = String::from_utf8_lossy(&bytes);
        assert!(
            msg.contains("gh"),
            "the refusal must name the gh CLI, got: {msg}"
        );
    }

    #[tokio::test]
    async fn detach_pull_request_404_for_unknown_session() {
        let (_tmp, app) = router_no_auth();
        let resp = send_json(&app, "DELETE", "/api/v1/sessions/ghost/pull-request", None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    }

    /// Detaching a session that never had a manual pin is a successful no-op
    /// (200); the honest "was already active" info rides the status stream.
    #[tokio::test]
    async fn detach_pull_request_200_without_override() {
        let (_tmp, app) = router_with_seeded_session("s1");
        let resp = send_json(&app, "DELETE", "/api/v1/sessions/s1/pull-request", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    }

    /// A CONFIRMED create (`use_existing_branch: true`) is not refused by the
    /// preflight even when the name matches an existing branch.
    #[tokio::test]
    async fn create_allows_confirmed_existing_branch_attach() {
        let (_tmp, app, project_id) = router_with_project_and_branch("feature-x");
        let resp = post_create(
            &app,
            serde_json::json!({
                "kind": "new",
                "project_id": project_id,
                "name": "feature-x",
                "use_existing_branch": true,
            }),
        )
        .await;
        assert_ne!(
            resp.status(),
            StatusCode::CONFLICT,
            "a confirmed attach must pass the preflight"
        );
    }
}

// ── Pull-request reference resolution ────────────────────────────────────────

/// What the client typed into the pull-request field.
#[derive(Deserialize)]
struct ResolvePullRequestBody {
    reference: String,
}

/// One project that is a checkout of the repository the reference names.
#[derive(Serialize)]
struct PullRequestProjectMatch {
    id: String,
    name: String,
}

/// The answer: what the reference turned out to name, which projects have it,
/// and whether the answer is complete. The client branches on `projects.len()`,
/// exactly as the terminal UI does: one proceeds, several ask, none reports and
/// offers the picker.
///
/// `uninspected` is what stops the "none" branch from claiming more than dux
/// knows. A project whose directory is gone, whose address git could not read,
/// or whose host `gh` is not signed in to was never compared, so it is not a
/// non-match: it is an unknown. Without this the client says "no project in dux
/// is a checkout of that repository" when the truth may be "the only project
/// that could have been was unreadable".
#[derive(Serialize)]
struct ResolvePullRequestReply {
    /// `host/owner/repo`, or `owner/repo` when the reference named no host.
    /// Absent for a bare number, which names no repository at all.
    repository: Option<String>,
    /// The pull request number, when the reference carried one.
    number: Option<u64>,
    projects: Vec<PullRequestProjectMatch>,
    /// How many projects dux could not inspect at all.
    uninspected_count: usize,
    /// A clause naming what could not be checked, already grouped by reason so
    /// the two surfaces word it identically. `null` when every project was
    /// inspected and "none of them" really means none of them.
    uninspected_summary: Option<String>,
}

/// Read a typed pull-request reference and match it against every project's
/// configured address.
///
/// The parse is pure and answers inline. The MATCH is one `git` call per
/// project, so it runs in `spawn_blocking` and never on the async reactor,
/// following the same rule every other git-shelling read here follows.
async fn resolve_pull_request_reference(
    State(state): State<AppState>,
    Json(body): Json<ResolvePullRequestBody>,
) -> Response {
    let reference = match dux_core::pr_reference::parse_typed_reference(&body.reference) {
        Ok(reference) => reference,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    // A bare number names no repository, so there is nothing to resolve and
    // nothing a git call could find. Refused here with the reason, rather than
    // answered with an empty match set that would read as "no project has it".
    if reference.owner_repo.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            "A pull request number on its own does not say which repository it is in. \
             Paste a link, type owner/repo#123, or choose an existing project first.",
        )
            .into_response();
    }

    let Some((projects, policy, gh_available)) =
        state.engine.pull_request_resolution_inputs().await
    else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "engine unavailable").into_response();
    };
    // The same gate the create carries: a raw or stale client must not reach a
    // resolution the create would then refuse.
    if !gh_available {
        return (
            StatusCode::BAD_REQUEST,
            "GitHub PR agent creation requires GitHub integration and an authenticated gh CLI.",
        )
            .into_response();
    }
    if let Some(host) = reference.host.as_deref()
        && !policy.allows(host)
    {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "dux cannot look up pull requests on {host}. Sign in to that host with \
                 `gh auth login --hostname {host}`, or paste a reference from a host you \
                 are already signed in to."
            ),
        )
            .into_response();
    }

    let repository = reference.repository_label();
    let number = reference.number;
    match tokio::task::spawn_blocking(move || {
        dux_core::pr_reference::resolve_reference_projects(&reference, &projects, &policy)
    })
    .await
    {
        Ok(resolution) => Json(ResolvePullRequestReply {
            repository,
            number,
            uninspected_count: resolution.uninspected.len(),
            uninspected_summary: resolution.uninspected_summary(),
            projects: resolution
                .matches
                .into_iter()
                .map(|project| PullRequestProjectMatch {
                    id: project.id,
                    name: project.name,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("resolving the pull request reference failed: {e}"),
        )
            .into_response(),
    }
}
