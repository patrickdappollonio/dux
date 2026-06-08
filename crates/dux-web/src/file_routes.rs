//! HTTP endpoints for the web code editor: read and write a worktree file's
//! working copy. Request/response so the editor gets real content + errors and
//! can drive per-file loading/saving state.
//!
//! Safety mirrors the git endpoints: both routes are auth-gated, run the file
//! I/O OFF the async reactor (`spawn_blocking`), and PRE-VALIDATE that the path
//! is a file git actually tracks/changes in this worktree (via
//! [`crate::git_routes::validate_changed_path`]) — which proves both "handled by
//! git" (excludes `.git/` internals and ignored files) and "inside the worktree
//! tree". `dux_core::worktree_file` adds a second, independent boundary
//! (`resolve_worktree_path`) that rejects path-escapes and symlinks, refuses to
//! create files on write, and rejects binary/oversized files on read.
//!
//! After a write, the engine recomputes changed files so the new state reaches
//! every connected client over the WebSocket ViewModel broadcast.

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;

use crate::git_routes::{resolve_worktree, validate_changed_path};
use crate::server::AppState;

#[derive(Deserialize)]
struct ReadOp {
    session_id: String,
    path: String,
}

#[derive(Deserialize)]
struct WriteOp {
    session_id: String,
    path: String,
    content: String,
}

/// The gated editor file routes, merged into the authenticated sub-router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/file/read", post(read_file))
        .route("/api/file/write", post(write_file))
}

async fn read_file(State(state): State<AppState>, Json(op): Json<ReadOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Err(r) = validate_changed_path(&worktree, &op.path).await {
        return r;
    }
    let path = op.path;
    match tokio::task::spawn_blocking(move || dux_core::worktree_file::read_file(&worktree, &path))
        .await
    {
        Ok(Ok(file)) => Json(file).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("read task failed: {e}"),
        )
            .into_response(),
    }
}

async fn write_file(State(state): State<AppState>, Json(op): Json<WriteOp>) -> Response {
    let worktree = match resolve_worktree(&state, op.session_id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Err(r) = validate_changed_path(&worktree, &op.path).await {
        return r;
    }
    let wt = worktree.clone();
    let path = op.path;
    let content = op.content;
    // write_file's errors are path/state validation (escape, symlink, not a
    // regular file) — client conditions, so map them to 400 like the read route,
    // not the 500 that `run_git` uses for genuine git-mutation failures.
    match tokio::task::spawn_blocking(move || {
        dux_core::worktree_file::write_file(&wt, &path, &content)
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("write task failed: {e}"),
            )
                .into_response();
        }
    }
    state
        .engine
        .refresh_changed_files(worktree.to_string_lossy().into_owned());
    StatusCode::OK.into_response()
}
