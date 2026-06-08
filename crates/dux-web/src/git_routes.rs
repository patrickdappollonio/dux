//! HTTP endpoints for mutating git operations: stage, unstage, discard, commit,
//! push, pull, and checkout-default.
//!
//! These are request/response so the web UI gets real completion + errors and
//! can drive per-action loading state. The WebSocket stays the channel for LIVE
//! changed-files broadcasts (after a mutation each handler asks the engine to
//! recompute, and the coalesced ViewModel watch pushes the new state to every
//! connected client).
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
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use dux_core::wire::WireCommand;
use serde::Deserialize;

use crate::server::AppState;

#[derive(Deserialize)]
struct FileOp {
    session_id: String,
    path: String,
}

#[derive(Deserialize)]
struct CommitOp {
    session_id: String,
    message: String,
}

#[derive(Deserialize)]
struct SessionOp {
    session_id: String,
}

#[derive(Deserialize)]
struct ProjectOp {
    project_id: String,
}

/// The gated git-mutation routes, merged into the authenticated sub-router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/git/stage", post(stage))
        .route("/api/git/unstage", post(unstage))
        .route("/api/git/discard", post(discard))
        .route("/api/git/commit", post(commit))
        .route("/api/git/push", post(push))
        .route("/api/git/pull", post(pull))
        .route("/api/git/pull-project", post(pull_project))
        .route("/api/git/checkout-default", post(checkout_default))
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
pub(crate) async fn validate_changed_path(worktree: &Path, path: &str) -> Result<(), Response> {
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

async fn stage(State(state): State<AppState>, Json(op): Json<FileOp>) -> Response {
    file_op(state, op.session_id, op.path, |wt, p| {
        dux_core::git::stage_file(&wt, &p)
    })
    .await
}

async fn unstage(State(state): State<AppState>, Json(op): Json<FileOp>) -> Response {
    file_op(state, op.session_id, op.path, |wt, p| {
        dux_core::git::unstage_file(&wt, &p)
    })
    .await
}

async fn discard(State(state): State<AppState>, Json(op): Json<FileOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
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
    StatusCode::OK.into_response()
}

async fn file_op<F>(state: AppState, session_id: String, path: String, op: F) -> Response
where
    F: FnOnce(PathBuf, String) -> anyhow::Result<()> + Send + 'static,
{
    let worktree = match resolve_worktree(&state, session_id).await {
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
    StatusCode::OK.into_response()
}

// ── Session-scoped ops (commit / push / pull) ────────────────────────────────

async fn commit(State(state): State<AppState>, Json(op): Json<CommitOp>) -> Response {
    if op.message.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "commit message is empty").into_response();
    }
    let worktree = match resolve_worktree(&state, op.session_id).await {
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
    StatusCode::OK.into_response()
}

// push / pull / pull-project / checkout-default are async, worker-based engine
// operations with stateful guards (in-flight dedup, the source-checkout dirty-
// tree refusal, leading-branch resolution, path-missing checks) and busy/done
// status. Rather than re-run raw git and lose all of that, these endpoints
// TRIGGER the existing engine command via `apply_wire` (which spawns the worker
// off the actor thread). A 200 means "accepted"; the busy/completion status
// flows to clients over the WebSocket status broadcast, exactly as before.

async fn push(State(state): State<AppState>, Json(op): Json<SessionOp>) -> Response {
    apply_wire_response(
        state
            .engine
            .apply_wire(WireCommand::Push {
                session_id: op.session_id,
            })
            .await,
    )
}

async fn pull(State(state): State<AppState>, Json(op): Json<SessionOp>) -> Response {
    apply_wire_response(
        state
            .engine
            .apply_wire(WireCommand::Pull {
                session_id: op.session_id,
            })
            .await,
    )
}

async fn pull_project(State(state): State<AppState>, Json(op): Json<ProjectOp>) -> Response {
    apply_wire_response(
        state
            .engine
            .apply_wire(WireCommand::PullProject {
                project_id: op.project_id,
            })
            .await,
    )
}

async fn checkout_default(State(state): State<AppState>, Json(op): Json<ProjectOp>) -> Response {
    apply_wire_response(
        state
            .engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: op.project_id,
            })
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
