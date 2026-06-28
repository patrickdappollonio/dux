//! `GET /api/v1/sessions/:id/changes` — the REST read for a session's changed
//! files, backed by [`crate::changes::ChangesService`].
//!
//! Status codes (per the REST-first design):
//! - 200 with a dedicated [`ChangesResponseBody`] (`{ rev, staged, unstaged }`).
//!   We deliberately do NOT serialize `dux_core::viewmodel::ChangedFilesView`: it
//!   carries the global `watched_session_id` we are removing and lacks `rev`.
//! - 404 when the session is unknown (no worktree), reusing `resolve_worktree`.
//! - 409 + `Retry-After` on a git lock/rebase error (logged first by the service);
//!   409 (not 503) because proxies may reroute a 503.
//!
//! Merged into the authenticated (gated) sub-router in `server.rs`, so an
//! unauthenticated request 401s before reaching this handler.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use dux_core::viewmodel::ChangedFileView;

use crate::changes::GitError;
use crate::git_routes::resolve_worktree;
use crate::server::AppState;

/// Upper bound on the `:id` path segment before any lookup (matches the
/// length-bounding convention for path params elsewhere).
const MAX_ID_LEN: usize = 128;

/// `Retry-After` seconds returned alongside a 409 so a client backs off before
/// refetching during a transient git lock/rebase.
const RETRY_AFTER_SECS: u64 = 2;

/// The dedicated 200 body. Distinct from `ChangedFilesView` (no global
/// `watched_session_id`; carries `rev`). The per-file [`ChangedFileView`] is reused.
#[derive(Serialize)]
struct ChangesResponseBody {
    rev: u64,
    staged: Vec<ChangedFileView>,
    unstaged: Vec<ChangedFileView>,
}

/// The gated changed-files read route.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/sessions/{id}/changes", get(get_changes))
}

async fn get_changes(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    // Length-bound the id before any lookup. Count characters, not bytes, so a
    // multi-byte id is not rejected early by its UTF-8 length.
    if id.chars().count() > MAX_ID_LEN {
        return (StatusCode::NOT_FOUND, "unknown session").into_response();
    }
    // 404 if the session is unknown (reuse the shared worktree resolver).
    if let Err(resp) = resolve_worktree(&state, id.clone()).await {
        return resp;
    }
    match state.changes.get(&id).await {
        Ok(c) => Json(ChangesResponseBody {
            rev: c.rev,
            staged: c.staged,
            unstaged: c.unstaged,
        })
        .into_response(),
        // The session vanished between the resolve and the read.
        Err(GitError::SessionNotFound) => {
            (StatusCode::NOT_FOUND, "unknown session").into_response()
        }
        // A git lock/rebase (or other git failure) — the service already logged it.
        Err(GitError::Git(_)) => (
            StatusCode::CONFLICT,
            [(header::RETRY_AFTER, RETRY_AFTER_SECS.to_string())],
            "changed files are temporarily unavailable (the repository is busy); retry shortly",
        )
            .into_response(),
    }
}
