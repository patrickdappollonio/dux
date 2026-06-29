//! HTTP endpoints for mutating git operations: stage, unstage, discard, commit,
//! push, and pull. Project-scoped git actions (source-checkout refresh and
//! checkout-default) live in [`crate::project_actions`].
//!
//! These are request/response so the web UI gets real completion + errors and
//! can drive per-action loading state. After a mutation each handler invalidates
//! the changed-files cache, which emits a `session.changes` event on `/ws/events`
//! so subscribed clients refetch `GET /api/v1/sessions/:id/changes`.
//!
//! Safety: every handler runs git OFF the engine actor thread AND off the async
//! reactor (`spawn_blocking`), so a slow/locked repo never stalls other clients.
//! File-path ops PRE-VALIDATE that the path is a file git actually tracks in the
//! worktree — `changed_files` only ever returns worktree-relative paths inside
//! the tree, so membership proves both "handled by git" and "inside the worktree
//! tree" (and unlike a filesystem canonicalize check it correctly accepts
//! deleted files, which appear in status but no longer exist on disk).
//!
//! All routes are merged into the authenticated (gated) sub-router in `server.rs`
//! — see [`crate::server`]'s router doc and `gated_data_route_is_401_without_session`.

use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    extract::{Path as ApiPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use dux_core::wire::WireCommand;
use serde::Deserialize;

use crate::rest_common::{id_within_bound, scope_from_headers, unknown_session};
use crate::server::AppState;

#[derive(Deserialize)]
struct FileOp {
    path: String,
}

#[derive(Deserialize)]
struct CommitOp {
    message: String,
}

/// Maximum number of Unicode scalar values in a commit message.
/// Git itself accepts messages up to ARG_MAX (~2 MiB on Linux), but very long
/// messages are almost always accidental. 64 KiB is generous for any real
/// commit message and guards against runaway clients.
const MAX_COMMIT_MSG_LEN: usize = 65_536;

/// The gated git-mutation routes, merged into the authenticated sub-router. These
/// are path-keyed: the session id is the `:id` path segment under
/// `/api/v1/sessions/:id/git/*`, validated by `id_within_bound` and then resolved
/// to a worktree at the top of each handler (mirroring the other resource-nested
/// REST routes). Project-scoped git actions (refresh the source checkout and
/// switch it to the default branch) live in [`crate::project_actions`] under the
/// path-keyed `/api/v1/projects/:id/{pull,checkout-default}` routes.
pub fn routes() -> Router<AppState> {
    let prefix = "/api/v1/sessions/{id}/git";
    Router::new()
        .route(&format!("{prefix}/stage"), post(stage))
        .route(&format!("{prefix}/unstage"), post(unstage))
        .route(&format!("{prefix}/discard"), post(discard))
        .route(&format!("{prefix}/commit"), post(commit))
        .route(&format!("{prefix}/push"), post(push))
        .route(&format!("{prefix}/pull"), post(pull))
}

pub(crate) async fn resolve_worktree(
    state: &AppState,
    session_id: String,
) -> Result<PathBuf, Response> {
    match state.engine.session_worktree(session_id).await {
        Some(w) => Ok(PathBuf::from(w)),
        None => Err((StatusCode::NOT_FOUND, "unknown session").into_response()),
    }
}

/// Reject a file path that isn't a real changed file git is tracking in this
/// worktree (defends against operating on arbitrary filesystem paths). Runs the
/// `git status` read off-thread.
async fn validate_changed_path(worktree: &Path, path: &str) -> Result<(), Response> {
    let wt = worktree.to_path_buf();
    let p = path.to_string();
    let ok = tokio::task::spawn_blocking(move || match dux_core::git::changed_files(&wt) {
        Ok((staged, unstaged)) => staged.iter().chain(&unstaged).any(|f| f.path == p),
        Err(_) => false,
    })
    .await
    .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            format!("not a changed file tracked by git in this worktree: {path}"),
        )
            .into_response())
    }
}

/// Run a blocking git closure off the reactor, mapping its result to a response
/// error (the success arm is left to the caller, which may also refresh state).
async fn run_git<F>(op: F) -> Result<(), Response>
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    match tokio::task::spawn_blocking(op).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("git task failed: {e}"),
        )
            .into_response()),
    }
}

// ── File-path ops (stage / unstage / discard) ────────────────────────────────

async fn stage(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Json(op): Json<FileOp>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    file_op(state, id, op.path, |wt, p| {
        dux_core::git::stage_file(&wt, &p)
    })
    .await
}

async fn unstage(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Json(op): Json<FileOp>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    file_op(state, id, op.path, |wt, p| {
        dux_core::git::unstage_file(&wt, &p)
    })
    .await
}

async fn discard(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Json(op): Json<FileOp>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    let session_id = id.clone();
    let worktree = match resolve_worktree(&state, id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    // Discard is destructive (deletes untracked files / restores tracked ones),
    // so the tracked-vs-untracked distinction is derived SERVER-SIDE from live
    // git status — never trusted from the client. This also rejects staged files
    // ("unstage first") and files with nothing to discard, with a message.
    let wt = worktree.clone();
    let p = op.path.clone();
    let untracked = match tokio::task::spawn_blocking(move || {
        dux_core::wire::discard_classify(&wt, &p)
    })
    .await
    {
        Ok(Ok(u)) => u,
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("git task failed: {e}"),
            )
                .into_response();
        }
    };
    let wt = worktree.clone();
    let path = op.path;
    if let Err(r) = run_git(move || dux_core::git::discard_file(&wt, &path, untracked)).await {
        return r;
    }
    state
        .engine
        .refresh_changed_files(worktree.to_string_lossy().into_owned());
    state.changes.invalidate(session_id);
    StatusCode::OK.into_response()
}

async fn file_op<F>(state: AppState, session_id: String, path: String, op: F) -> Response
where
    F: FnOnce(PathBuf, String) -> anyhow::Result<()> + Send + 'static,
{
    let worktree = match resolve_worktree(&state, session_id.clone()).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Err(r) = validate_changed_path(&worktree, &path).await {
        return r;
    }
    let wt = worktree.clone();
    if let Err(r) = run_git(move || op(wt, path)).await {
        return r;
    }
    state
        .engine
        .refresh_changed_files(worktree.to_string_lossy().into_owned());
    // Refresh the REST changed-files cache (new path) immediately too, emitting
    // `session.changes` so subscribed `/ws/events` clients re-GET without waiting
    // for the poll interval.
    state.changes.invalidate(session_id);
    StatusCode::OK.into_response()
}

// ── Session-scoped ops (commit / push / pull) ────────────────────────────────

async fn commit(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Json(op): Json<CommitOp>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    if op.message.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "commit message is empty").into_response();
    }
    if op.message.chars().count() > MAX_COMMIT_MSG_LEN {
        return (
            StatusCode::BAD_REQUEST,
            format!("commit message exceeds the {MAX_COMMIT_MSG_LEN}-character limit"),
        )
            .into_response();
    }
    let session_id = id.clone();
    let worktree = match resolve_worktree(&state, id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let wt = worktree.clone();
    let message = op.message;
    if let Err(r) = run_git(move || dux_core::git::commit(&wt, &message).map(|_| ())).await {
        return r;
    }
    state
        .engine
        .refresh_changed_files(worktree.to_string_lossy().into_owned());
    state.changes.invalidate(session_id);
    StatusCode::OK.into_response()
}

// push / pull are async, worker-based engine operations with stateful guards
// (in-flight dedup, leading-branch resolution) and busy/done status. Rather than
// re-run raw git and lose all of that, these endpoints TRIGGER the existing engine
// command via `apply_wire` (which spawns the worker off the actor thread). A 200
// means "accepted"; the busy/completion status flows to the originating client as
// `status` events on `/ws/events` (scoped via the `X-Connection-Id` header).

async fn push(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    apply_wire_response(
        state
            .engine
            .apply_wire_scoped(
                WireCommand::Push { session_id: id },
                scope_from_headers(&headers, &state.connections),
            )
            .await,
    )
}

async fn pull(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    apply_wire_response(
        state
            .engine
            .apply_wire_scoped(
                WireCommand::Pull { session_id: id },
                scope_from_headers(&headers, &state.connections),
            )
            .await,
    )
}

/// Map an `apply_wire` result to an HTTP response. `Ok` = the command was
/// accepted (its busy/success status and async worker completion reach clients
/// over the WS status broadcast); `Err` is a synchronous resolution/guard
/// refusal (unknown session/project, source checkout path missing, …).
fn apply_wire_response(result: Result<dux_core::wire::WireCommandOutcome, String>) -> Response {
    match result {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::test_support::router_no_auth;

    fn json_req(method: &str, uri: &str, body: &str) -> Request<axum::body::Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn commit_rejects_over_length_message_with_400() {
        let (_tmp, app) = router_no_auth();
        // Build a message one character over the cap using 'a' (1-byte ASCII
        // so chars().count() == len(), making the boundary explicit).
        let long_msg = "a".repeat(MAX_COMMIT_MSG_LEN + 1);
        let body = format!(r#"{{"message":"{long_msg}"}}"#);
        let resp = app
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/abc123/git/commit",
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn commit_accepts_message_at_exactly_the_length_cap() {
        let (_tmp, app) = router_no_auth();
        let ok_msg = "a".repeat(MAX_COMMIT_MSG_LEN);
        let body = format!(r#"{{"message":"{ok_msg}"}}"#);
        let resp = app
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/abc123/git/commit",
                &body,
            ))
            .await
            .unwrap();
        // The at-cap message passes the length gate; the session does not exist,
        // so the handler returns 404 from the worktree lookup rather than 400.
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "at-cap message must pass the length gate and reach the session lookup (404)"
        );
    }

    /// A commit message of exactly MAX_COMMIT_MSG_LEN MULTI-BYTE characters must
    /// not be rejected with 400. Proves the cap uses `.chars().count()` rather than
    /// `.len()` (a 2-byte char like 'e with acute' has byte length > char count).
    #[tokio::test]
    async fn commit_accepts_multibyte_message_at_exactly_the_length_cap() {
        let (_tmp, app) = router_no_auth();
        // 'é' is 2 UTF-8 bytes; MAX_COMMIT_MSG_LEN copies = MAX_COMMIT_MSG_LEN
        // chars but 2*MAX_COMMIT_MSG_LEN bytes. A byte-based cap would reject this.
        let ok_msg = "é".repeat(MAX_COMMIT_MSG_LEN);
        let body = format!(r#"{{"message":"{ok_msg}"}}"#);
        let resp = app
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/abc123/git/commit",
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "multi-byte at-cap message must pass the length gate (cap is chars, not bytes)"
        );
    }
}
