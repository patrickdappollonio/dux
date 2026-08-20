//! HTTP endpoints for mutating git operations: stage, unstage, discard, commit,
//! push, and pull. Project-scoped git actions (source-checkout refresh and
//! checkout-default) live in [`crate::project_actions`].
//!
//! These are request/response so the web UI gets real completion + errors and
//! can drive per-action loading state. After a mutation each handler invalidates
//! the changed-files cache, which emits a `session.changes` event on `/ws/events`
//! so subscribed clients refetch `GET /api/v1/sessions/:id/changes`.
//!
//! `refresh-changes` is the one route here that mutates nothing: it performs
//! only that post-mutation refresh, so a change dux did not make through one of
//! these routes (a file the user changed from a terminal, or an agent writing in
//! its worktree) can be picked up now instead of on the next poll. See
//! [`refresh_changed_files_now`], which every handler in this module, every
//! handler in [`crate::file_routes`], and the file-drop upload in
//! [`crate::file_drop_routes`] share so they can never drift apart.
//!
//! Safety: every handler runs git OFF the engine actor thread AND off the async
//! reactor (`spawn_blocking`), so a slow/locked repo never stalls other clients.
//! File-path ops PRE-VALIDATE that the path is a file git actually tracks in the
//! worktree — `changed_files` only ever returns worktree-relative paths inside
//! the tree, so membership proves both "handled by git" and "inside the worktree
//! tree" (and unlike a filesystem canonicalize check it correctly accepts
//! deleted files, which appear in status but no longer exist on disk).
//!
//! All routes are served plainly, like every other API route. dux has NO
//! authentication of any kind, so nothing here ever 401s: the open access is
//! deliberate, the single-tenant trusted-access model documented in CLAUDE.md.
//! Two app-wide guards apply (see [`crate::server::build_app`]'s middleware
//! doc): a Host-header allowlist, which stops a malicious web page from rebinding
//! DNS into this server, and the same-origin check on mutating verbs, which stops
//! another site driving these POSTs from a visitor's browser. Neither is
//! authentication, and a client that sends no `Origin` header (curl, a script)
//! bypasses the origin check by design, so any client that can reach the address
//! can commit, push and discard in every worktree.

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

/// The git-mutation routes. These
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
        .route(&format!("{prefix}/refresh-changes"), post(refresh_changes))
}

/// Recompute a session's changed files NOW: the exact pair of calls every
/// mutating handler in this module makes after it touches a file.
///
/// dux has no file watcher: the cached answer is dropped by the routes that
/// change a file, which is every handler here, every handler in
/// [`crate::file_routes`], and the file-drop upload in
/// [`crate::file_drop_routes`] when the dropped file lands inside the agent's
/// worktree. Anything dux did not do through one of them only catches up on the
/// next poll (2s while any agent or terminal in the workspace is running, 10s
/// while none is).
///
/// Both halves are needed and neither is redundant: the engine call refreshes
/// the lists the engine itself serves, and the invalidate drops the REST cache
/// entry so the next GET recomputes rather than re-serving the pre-edit
/// snapshot.
pub(crate) fn refresh_changed_files_now(state: &AppState, session_id: String, worktree: &Path) {
    state
        .engine
        .refresh_changed_files(worktree.to_string_lossy().into_owned());
    // Emits `session.changes` so subscribed `/ws/events` clients re-GET without
    // waiting for the poll interval.
    state.changes.invalidate(session_id);
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
///
/// `action` names what was attempted, in a form that reads inside a sentence
/// ("could not stage the file"), and PREFIXES git's own message rather than
/// replacing it. The client is told both.
///
/// An earlier version returned the action alone and sent the reader to
/// `dux.log`. That was wrong twice over. It is not actionable: the preflight
/// upstream covers exactly two cases (empty message, nothing staged), and
/// everything else arrives here with the explanation the user needs and nothing
/// to do with it, including a `pre-commit` or `commit-msg` hook's report (the
/// entire reason a hook prints anything), `gpg failed to sign the data`,
/// "Committing is not possible because you have unmerged files" which carries
/// its own fix instruction, and a held `index.lock` whose message literally
/// says how to clear it. And on a remote browser `dux.log` is on a machine the
/// reader may have no way to reach. It was also inconsistent: push and pull
/// deliver git's full text to this same browser through the status toast, so a
/// failed push explained itself and a failed commit did not. The project's own
/// rule is that messages are verbose and actionable.
///
/// The server's worktree path is stripped by
/// [`dux_core::git::redact_worktree_path`], which is the part of the old
/// reasoning that was worth keeping: the browser may be on another machine
/// entirely, where the server's directory layout is noise. That is a tidiness
/// measure, not a security boundary. dux is single-tenant and loopback by
/// default, and the same redaction is applied at the source for commit, push
/// and pull so all three read the same way on both surfaces. The full,
/// unredacted chain still goes to `dux.log` for the operator.
///
/// The ordinary refusals a user actually hits do not come through here: the
/// empty-message and nothing-staged commit cases are caught by
/// `git::commit_preflight` and answered as a 400 with their own wording.
async fn run_git<F>(action: &'static str, worktree: &Path, op: F) -> Result<(), Response>
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    let worktree = worktree.to_path_buf();
    match tokio::task::spawn_blocking(op).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            dux_core::logger::warn(&format!("[web] could not {action}: {e:#}"));
            let detail = dux_core::git::redact_worktree_path(&format!("{e:#}"), &worktree);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Could not {action}. {detail}"),
            )
                .into_response())
        }
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
    file_op(state, id, op.path, "stage the file", |wt, p| {
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
    file_op(state, id, op.path, "unstage the file", |wt, p| {
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
    let untracked =
        match tokio::task::spawn_blocking(move || dux_core::git::discard_classify(&wt, &p)).await {
            Ok(Ok(u)) => u,
            // `discard_classify`'s refusals are written to be read ("unstage
            // first", "nothing to discard"), so they go to the client as they
            // always have; the path redaction is applied for the same reason
            // `run_git` applies it.
            Ok(Err(e)) => {
                return (
                    StatusCode::BAD_REQUEST,
                    dux_core::git::redact_worktree_path(&e.to_string(), &worktree),
                )
                    .into_response();
            }
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
    if let Err(r) = run_git("discard the file's changes", &worktree, move || {
        dux_core::git::discard_file(&wt, &path, untracked)
    })
    .await
    {
        return r;
    }
    refresh_changed_files_now(&state, session_id, &worktree);
    StatusCode::OK.into_response()
}

async fn file_op<F>(
    state: AppState,
    session_id: String,
    path: String,
    action: &'static str,
    op: F,
) -> Response
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
    if let Err(r) = run_git(action, &worktree, move || op(wt, path)).await {
        return r;
    }
    refresh_changed_files_now(&state, session_id, &worktree);
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
    // Length is a web payload bound (not a git semantic), so it stays a cheap
    // pre-check here rather than moving into the shared core preflight.
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
    // The empty-message and nothing-staged refusals are the shared core decision
    // (`commit_preflight`), read against LIVE git status. This adds the
    // nothing-staged gate the web previously lacked: a stale commit with nothing
    // staged used to reach `git commit` and 500 with raw stderr; it now returns a
    // clean 400. Each surface renders its own copy for these refusals.
    let wt = worktree.clone();
    let msg = op.message.clone();
    let preflight =
        match tokio::task::spawn_blocking(move || dux_core::git::commit_preflight(&wt, &msg)).await
        {
            Ok(p) => p,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("git task failed: {e}"),
                )
                    .into_response();
            }
        };
    match preflight {
        dux_core::git::CommitPreflight::EmptyMessage => {
            return (StatusCode::BAD_REQUEST, "commit message is empty").into_response();
        }
        dux_core::git::CommitPreflight::NothingStaged => {
            return (StatusCode::BAD_REQUEST, "no staged changes to commit").into_response();
        }
        dux_core::git::CommitPreflight::Ready => {}
    }
    let wt = worktree.clone();
    let message = op.message;
    if let Err(r) = run_git("commit the staged changes", &worktree, move || {
        dux_core::git::commit(&wt, &message).map(|_| ())
    })
    .await
    {
        return r;
    }
    refresh_changed_files_now(&state, session_id, &worktree);
    StatusCode::OK.into_response()
}

/// `POST /api/v1/sessions/:id/git/refresh-changes` — recompute this session's
/// changed files now.
///
/// dux invalidates its cached answer whenever DUX changes a file, but it cannot
/// see a file the user changed from a terminal, so this is how the user says
/// "look again" instead of waiting out the poll interval. It changes nothing on
/// disk; it only forces the read that every mutating handler here forces.
async fn refresh_changes(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    let session_id = id.clone();
    let worktree = match resolve_worktree(&state, id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    refresh_changed_files_now(&state, session_id, &worktree);
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

    /// A router whose session `s1` points at a real git repo, plus a clone of the
    /// live [`AppState`]. The state is captured through a probe route (the
    /// `extra_gated` hook `build_app` exposes for exactly this), which is the only
    /// way to reach the changes cache and the engine handle a real request sees.
    async fn router_with_session_and_state() -> (tempfile::TempDir, Router, AppState) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        // The git repo lives in its own subdir so the dux runtime files at `root`
        // never show up as untracked changes.
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        run_git(&wt, &["init", "-q"]);
        run_git(&wt, &["config", "user.email", "t@example.com"]);
        run_git(&wt, &["config", "user.name", "t"]);
        std::fs::write(wt.join("f.txt"), "line1\n").unwrap();
        run_git(&wt, &["add", "f.txt"]);
        run_git(&wt, &["commit", "-q", "-m", "init"]);

        let paths = dux_core::config::DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .upsert_project(&dux_core::config::ProjectConfig {
                    id: "p1".to_string(),
                    path: root.to_string_lossy().into_owned(),
                    name: Some("p1".to_string()),
                    default_provider: None,
                    leading_branch: None,
                    auto_reopen_agents: None,
                    startup_command: None,
                    env: Default::default(),
                })
                .unwrap();
            store
                .upsert_session(&sample_session("s1", wt.to_string_lossy().as_ref()))
                .unwrap();
        }
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);

        let slot: std::sync::Arc<std::sync::Mutex<Option<AppState>>> = Default::default();
        let captured = std::sync::Arc::clone(&slot);
        let probe = Router::new().route(
            "/test/state",
            axum::routing::get(move |State(state): State<AppState>| {
                let captured = std::sync::Arc::clone(&captured);
                async move {
                    *captured.lock().unwrap() = Some(state);
                    "ok"
                }
            }),
        );
        let app =
            crate::server::build_app(handle, probe, crate::server::RouterParams::plain_http());
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/test/state")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let state = slot.lock().unwrap().take().expect("probe captured state");
        (tmp, app, state)
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git {args:?} failed");
    }

    fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
        let now = chrono::Utc::now();
        dux_core::model::AgentSession {
            id: id.to_string(),
            provider: dux_core::model::ProviderKind::new("claude"),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: false,
            status: dux_core::model::SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: "p1".to_string(),
                    project_path: None,
                    source_branch: "main".to_string(),
                    branch_name: "feat".to_string(),
                    initial_branch: "feat".to_string(),
                    branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                    worktree_path: worktree.to_string(),
                },
            ),
        }
    }

    /// The refresh route has to do BOTH halves of what every mutating handler
    /// above does after it touches a file: ask the engine to recompute its own
    /// lists, and drop the REST cache entry so the next GET recomputes instead of
    /// re-serving the answer from before the user edited anything in a terminal.
    /// Doing only one of them looks like it worked and changes nothing.
    #[tokio::test]
    async fn refresh_changes_invalidates_the_cache_and_asks_the_engine_to_refresh() {
        let (_tmp, app, state) = router_with_session_and_state().await;

        // Prime the cache so there is a stale entry to drop.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/sessions/s1/changes")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let generation_before = state.changes.invalidation_generation();
        assert!(
            state.engine.refresh_requests().is_empty(),
            "nothing has asked the engine to refresh yet"
        );

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/refresh-changes",
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        assert!(
            state.changes.invalidation_generation() > generation_before,
            "the REST changed-files cache must be invalidated, or the next GET \
             serves the same stale answer"
        );
        let refreshes = state.engine.refresh_requests();
        assert_eq!(
            refreshes.len(),
            1,
            "the engine must be asked to recompute exactly once, got {refreshes:?}"
        );
        assert!(
            refreshes[0].ends_with("wt"),
            "the refresh must name the session's own worktree, got {:?}",
            refreshes[0]
        );
    }

    /// Same unknown-session behaviour as its neighbours in this module: a 404 from
    /// the shared worktree resolver.
    #[tokio::test]
    async fn refresh_changes_unknown_session_is_404() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/does-not-exist/git/refresh-changes",
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// An over-long id is refused before any lookup, exactly like stage/unstage.
    #[tokio::test]
    async fn refresh_changes_over_long_id_is_404() {
        let (_tmp, app) = router_no_auth();
        let id = "a".repeat(crate::rest_common::MAX_ID_LEN + 1);
        let resp = app
            .oneshot(json_req(
                "POST",
                &format!("/api/v1/sessions/{id}/git/refresh-changes"),
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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

    /// The discard route classifies the file itself, BEFORE `run_git`, so its
    /// error arm carries its own redaction rather than inheriting one. Removing
    /// that call left the whole `dux-web` suite green, which is why this test
    /// exists.
    ///
    /// The failure is built by deleting the worktree out from under the route:
    /// `git status -C <gone>` names the missing directory by absolute path on
    /// stderr, and `changed_files` passes that text through, so the server's
    /// layout would reach a browser that may be on another machine entirely.
    #[tokio::test]
    async fn discard_strips_the_server_path_from_a_classify_refusal() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::remove_dir_all(&worktree).unwrap();

        let resp = app
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"f.txt"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(
            body.contains("git status failed"),
            "the reason must survive the redaction: {body}"
        );
        assert!(
            !body.contains(worktree.to_string_lossy().as_ref()),
            "the response must not carry the server's worktree path: {body}"
        );
    }

    /// A failing git helper must tell the browser WHY. The action alone plus a
    /// pointer to `dux.log` is not actionable, and on a remote browser that log
    /// is on a machine the reader may not be able to reach. The server's
    /// worktree path is still stripped, because the browser has no use for it.
    #[tokio::test]
    async fn run_git_reports_gits_reason_with_the_action_and_without_the_server_path() {
        let worktree = PathBuf::from("/home/someone/.config/dux/worktrees/proj/agent");
        let stderr = "error: 'trailing-whitespace' hook failed; \
                      see /home/someone/.config/dux/worktrees/proj/agent/out.log";
        let err = super::run_git("commit the staged changes", &worktree, move || {
            Err(anyhow::anyhow!("git commit failed: {stderr}"))
        })
        .await
        .expect_err("a failing git op must produce a response");

        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = String::from_utf8(
            axum::body::to_bytes(err.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(
            body.contains("commit the staged changes"),
            "the response must still name what failed: {body}"
        );
        assert!(
            body.contains("'trailing-whitespace' hook failed"),
            "the response must carry git's reason: {body}"
        );
        assert!(
            !body.contains("/home/someone"),
            "the response must not carry a server path: {body}"
        );
        assert!(
            body.contains("./out.log"),
            "the path should be relative to the worktree, not dropped: {body}"
        );
    }
}
