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

use crate::rest_common::{RouteRejection, id_within_bound, scope_from_headers, unknown_session};
use crate::server::AppState;

#[derive(Deserialize)]
struct FileOp {
    path: String,
}

/// A batch of worktree-relative paths for the stage-files / unstage-files
/// routes.
#[derive(Deserialize)]
struct FilesOp {
    paths: Vec<String>,
}

/// What a batch route answers with: the paths it acted on, and the paths that
/// were no longer in the section it validates against. A path that moved
/// between the click and the request must not take the rest of the batch down
/// with it.
#[derive(serde::Serialize)]
struct BatchResult {
    done: Vec<String>,
    refused: Vec<String>,
}

/// Maximum number of paths one batch may name. `changed_files` runs
/// `--untracked-files=all`, so a select-all in a repository with a large
/// untracked tree can reach tens of thousands of paths; the cap keeps one
/// request bounded and is answered with a sentence rather than a bare status.
const MAX_BATCH_PATHS: usize = 2_000;

/// Body cap for the batch routes. Comfortably holds `MAX_BATCH_PATHS` long
/// paths and stops a client streaming a multi-megabyte body at them.
const MAX_BATCH_BODY_BYTES: usize = 1024 * 1024;

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
        .route(
            &format!("{prefix}/stage-files"),
            post(stage_files).layer(axum::extract::DefaultBodyLimit::max(MAX_BATCH_BODY_BYTES)),
        )
        .route(
            &format!("{prefix}/unstage-files"),
            post(unstage_files).layer(axum::extract::DefaultBodyLimit::max(MAX_BATCH_BODY_BYTES)),
        )
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
) -> Result<PathBuf, RouteRejection> {
    match state.engine.session_worktree(session_id).await {
        Some(w) => Ok(PathBuf::from(w)),
        None => Err((StatusCode::NOT_FOUND, "unknown session")
            .into_response()
            .into()),
    }
}

/// Resolve the directory a CHANGES-PANEL route may run git in: a managed
/// worktree, or a standalone agent's folder when that folder is itself a
/// repository.
///
/// Folder-driven and not agent-driven, deliberately: a standalone agent pointed
/// at a repository gets a real changes panel. When the folder is not one, the
/// refusal carries the folder's OWN sentence ("this folder has no git
/// repository", "this folder sits inside a repository rooted elsewhere"), never
/// a git error about a repository nobody named.
///
/// `409` rather than `404`, because the agent exists and the route is real: it
/// is the folder underneath that cannot answer, which is the same shape as a
/// locked repository and the status the client already knows how to render.
pub(crate) async fn resolve_changes_worktree(
    state: &AppState,
    session_id: String,
) -> Result<PathBuf, RouteRejection> {
    resolve_git_directory(state, session_id, GitAsk::Read).await
}

/// The same directory for a route that WRITES (stage, unstage, discard,
/// commit), gated on the engine's mutation predicate instead.
///
/// A separate entry point because it is a separate question: the two predicates
/// answer identically today, and a read-only repository view would show files it
/// must not let anyone stage. Asking the read question in a mutating handler is
/// how that difference would go unnoticed the day it appears.
pub(crate) async fn resolve_mutation_worktree(
    state: &AppState,
    session_id: String,
) -> Result<PathBuf, RouteRejection> {
    resolve_git_directory(state, session_id, GitAsk::Mutate).await
}

/// Which of the two engine predicates a resolution asks.
#[derive(Clone, Copy)]
enum GitAsk {
    Read,
    Mutate,
}

async fn resolve_git_directory(
    state: &AppState,
    session_id: String,
    ask: GitAsk,
) -> Result<PathBuf, RouteRejection> {
    let access = match state.engine.session_git_access(session_id).await {
        Some(access) => access,
        None => {
            return Err((StatusCode::NOT_FOUND, "unknown session")
                .into_response()
                .into());
        }
    };
    let allowed = match ask {
        GitAsk::Read => access.changes_panel_works(),
        GitAsk::Mutate => access.mutations_allowed(),
    };
    if allowed {
        return Ok(access.directory().to_path_buf());
    }
    Err((
        StatusCode::CONFLICT,
        access
            .quiet_reason()
            .unwrap_or("dux cannot work with git in this folder.")
            .to_string(),
    )
        .into_response()
        .into())
}

/// Reject a file path that isn't a real changed file git is tracking in this
/// worktree (defends against operating on arbitrary filesystem paths). Runs the
/// `git status` read off-thread.
async fn validate_changed_path(worktree: &Path, path: &str) -> Result<(), RouteRejection> {
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
            .into_response()
            .into())
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
async fn run_git<F>(action: &'static str, worktree: &Path, op: F) -> Result<(), RouteRejection>
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
                .into_response()
                .into())
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("git task failed: {e}"),
        )
            .into_response()
            .into()),
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
    let worktree = match resolve_mutation_worktree(&state, id).await {
        Ok(w) => w,
        Err(r) => return r.into_response(),
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
        return r.into_response();
    }
    refresh_changed_files_now(&state, session_id, &worktree);
    StatusCode::OK.into_response()
}

async fn stage_files(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Json(op): Json<FilesOp>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    files_op(state, id, op.paths, Section::Unstaged).await
}

async fn unstage_files(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Json(op): Json<FilesOp>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    files_op(state, id, op.paths, Section::Staged).await
}

/// Which changes-pane section a batch is validated against, which decides both
/// the git verb and what "no longer there" means.
#[derive(Clone, Copy)]
enum Section {
    Staged,
    Unstaged,
}

impl Section {
    fn word(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Staged => "unstage the files",
            Self::Unstaged => "stage the files",
        }
    }
}

/// Stage or unstage a whole batch: one validating `git status` read, one git
/// call, one changed-files refresh.
///
/// The batch is PARTITIONED rather than refused whole. Validation is
/// section-scoped, because the two verbs mean opposite things: staging is
/// offered for a file in the unstaged list and unstaging for one in the staged
/// list, and a path that left its section between the click and the request is
/// reported in `refused` while the rest proceed. Only an empty present set is a
/// 400.
///
/// Partitioning does not make the git call itself partial: the present subset
/// runs as one batch, so a path that vanishes between the status read and the
/// git call fails the whole batch with git's own error rather than turning into
/// another `refused` entry.
async fn files_op(
    state: AppState,
    session_id: String,
    paths: Vec<String>,
    section: Section,
) -> Response {
    if paths.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "no files were named, so there is nothing to do".to_string(),
        )
            .into_response();
    }
    if paths.len() > MAX_BATCH_PATHS {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "{} files were named, which is more than the {MAX_BATCH_PATHS} one request may \
                 carry. Select fewer files, or filter the list and act on it in batches.",
                paths.len()
            ),
        )
            .into_response();
    }
    let worktree = match resolve_mutation_worktree(&state, session_id.clone()).await {
        Ok(w) => w,
        Err(r) => return r.into_response(),
    };

    let wt = worktree.clone();
    let requested = paths.clone();
    let partition = tokio::task::spawn_blocking(move || {
        dux_core::git::changed_files(&wt).map(|(staged, unstaged)| {
            let live: std::collections::HashSet<&str> = match section {
                Section::Staged => staged.iter().map(|f| f.path.as_str()).collect(),
                Section::Unstaged => unstaged.iter().map(|f| f.path.as_str()).collect(),
            };
            let mut seen = std::collections::HashSet::new();
            let mut done = Vec::new();
            let mut refused = Vec::new();
            for path in requested {
                if !seen.insert(path.clone()) {
                    continue;
                }
                if live.contains(path.as_str()) {
                    done.push(path);
                } else {
                    refused.push(path);
                }
            }
            (done, refused)
        })
    })
    .await;
    let (done, refused) = match partition {
        Ok(Ok(split)) => split,
        Ok(Err(e)) => {
            dux_core::logger::warn(&format!("[web] could not read changed files: {e:#}"));
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "Could not read this worktree's changed files. {}",
                    dux_core::git::redact_worktree_path(&format!("{e:#}"), &worktree)
                ),
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
    if done.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "none of the {} selected files are in this worktree's {} changes any more \
                 (starting with \"{}\"). Refresh the changes and try again.",
                refused.len(),
                section.word(),
                refused.first().map(String::as_str).unwrap_or_default(),
            ),
        )
            .into_response();
    }

    let wt = worktree.clone();
    let batch = done.clone();
    if let Err(r) = run_git(section.action(), &worktree, move || match section {
        Section::Staged => dux_core::git::unstage_files(&wt, &batch),
        Section::Unstaged => dux_core::git::stage_files(&wt, &batch),
    })
    .await
    {
        return r.into_response();
    }
    refresh_changed_files_now(&state, session_id, &worktree);
    (StatusCode::OK, Json(BatchResult { done, refused })).into_response()
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
    let worktree = match resolve_mutation_worktree(&state, session_id.clone()).await {
        Ok(w) => w,
        Err(r) => return r.into_response(),
    };
    if let Err(r) = validate_changed_path(&worktree, &path).await {
        return r.into_response();
    }
    let wt = worktree.clone();
    if let Err(r) = run_git(action, &worktree, move || op(wt, path)).await {
        return r.into_response();
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
    let worktree = match resolve_mutation_worktree(&state, id).await {
        Ok(w) => w,
        Err(r) => return r.into_response(),
    };
    // The empty-message and nothing-staged refusals are the shared core decision
    // (`commit_preflight`), read against LIVE git status. The nothing-staged gate
    // turns a stale commit with nothing staged into a clean 400 instead of letting
    // it reach `git commit` and 500 with raw stderr. Each surface renders its own
    // copy for these refusals.
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
        return r.into_response();
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
    let worktree = match resolve_changes_worktree(&state, id).await {
        Ok(w) => w,
        Err(r) => return r.into_response(),
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
            // A standalone agent in a plain directory: no repository, so every
            // mutating route must be refused by the workspace chokepoint.
            let plain = root.join("plain");
            std::fs::create_dir_all(&plain).unwrap();
            store
                .upsert_session(&standalone_session("sa1", plain.to_string_lossy().as_ref()))
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

    fn standalone_session(id: &str, folder: &str) -> dux_core::model::AgentSession {
        let mut session = sample_session(id, folder);
        session.workspace =
            dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
                folder_path: folder.to_string(),
            });
        session
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
        .expect_err("a failing git op must produce a response")
        .into_response();

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

    async fn body_text(resp: Response) -> String {
        String::from_utf8(
            axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    /// Dirty three working-tree files in the fixture worktree, one of them named
    /// so git would read it as an option.
    fn dirty_three(worktree: &Path) {
        std::fs::write(worktree.join("f.txt"), "line1\nline2\n").unwrap();
        std::fs::write(worktree.join("second.txt"), "new\n").unwrap();
        std::fs::write(worktree.join("-lead.txt"), "new\n").unwrap();
    }

    /// The batch route does in ONE call what N single-path calls did: one git
    /// invocation, one changed-files refresh, one broadcast. A per-path loop
    /// would refresh N times and the pane would churn.
    #[tokio::test]
    async fn stage_files_stages_every_named_path_and_refreshes_once() {
        let (tmp, app, state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        dirty_three(&worktree);

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                r#"{"paths":["f.txt","second.txt","-lead.txt"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(
            body.contains("f.txt"),
            "the response lists what it did: {body}"
        );
        assert!(
            body.contains("-lead.txt"),
            "an option-looking path is a path, not a flag: {body}"
        );

        let staged = tokio::task::spawn_blocking(move || {
            let (staged, _) = dux_core::git::changed_files(&worktree).unwrap();
            let mut paths: Vec<String> = staged.into_iter().map(|f| f.path).collect();
            paths.sort();
            paths
        })
        .await
        .unwrap();
        assert_eq!(
            staged,
            vec![
                "-lead.txt".to_string(),
                "f.txt".to_string(),
                "second.txt".to_string()
            ],
        );
        assert_eq!(
            state.engine.refresh_requests().len(),
            1,
            "a batch must refresh the changed files exactly once",
        );
    }

    /// The unstage batch is the mirror image: it names what it reset, leaves
    /// nothing in `refused`, and refreshes the changed files exactly once.
    #[tokio::test]
    async fn unstage_files_unstages_every_named_path_and_refreshes_once() {
        let (tmp, app, state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        dirty_three(&worktree);
        run_git(&worktree, &["add", "--", "f.txt", "second.txt"]);

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/unstage-files",
                r#"{"paths":["f.txt","second.txt"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert_eq!(parsed["done"], serde_json::json!(["f.txt", "second.txt"]));
        assert_eq!(parsed["refused"], serde_json::json!([]));

        let staged = tokio::task::spawn_blocking(move || {
            let (staged, _) = dux_core::git::changed_files(&worktree).unwrap();
            staged.into_iter().map(|f| f.path).collect::<Vec<_>>()
        })
        .await
        .unwrap();
        assert!(
            staged.is_empty(),
            "both paths should have left the index: {staged:?}"
        );
        assert_eq!(
            state.engine.refresh_requests().len(),
            1,
            "a batch must refresh the changed files exactly once",
        );
    }

    /// The batch routes carry their own body limit, so an oversized request is
    /// rejected by the layer before any handler allocates it.
    #[tokio::test]
    async fn a_batch_body_over_the_size_cap_is_rejected() {
        let (_tmp, app, _state) = router_with_session_and_state().await;
        let filler = "x".repeat(MAX_BATCH_BODY_BYTES + 1);
        let body = serde_json::json!({ "paths": [filler] }).to_string();
        assert!(body.len() > MAX_BATCH_BODY_BYTES);
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// A path that left the section between the click and the request must not
    /// take the rest of the batch down with it: the route acts on what it can
    /// and says what it could not.
    #[tokio::test]
    async fn stage_files_partitions_and_names_what_it_refused() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        dirty_three(&worktree);

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                r#"{"paths":["f.txt","ghost.txt"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["done"], serde_json::json!(["f.txt"]));
        assert_eq!(parsed["refused"], serde_json::json!(["ghost.txt"]));
    }

    /// Section-scoped: unstage validates against the STAGED list, so a file that
    /// is merely modified is refused rather than quietly reset.
    #[tokio::test]
    async fn unstage_files_validates_against_the_staged_section() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        dirty_three(&worktree);

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/unstage-files",
                r#"{"paths":["f.txt"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_text(resp).await;
        assert!(
            body.contains("f.txt"),
            "the refusal must name the path it could not act on: {body}"
        );
    }

    /// An empty list is a client bug, and git reads "no pathspec" as the whole
    /// index, so it never reaches git.
    #[tokio::test]
    async fn stage_files_refuses_an_empty_list() {
        let (_tmp, app, _state) = router_with_session_and_state().await;
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                r#"{"paths":[]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn stage_files_refuses_a_batch_over_the_count_cap_with_a_sentence() {
        let (_tmp, app, _state) = router_with_session_and_state().await;
        let paths: Vec<String> = (0..MAX_BATCH_PATHS + 1)
            .map(|i| format!("f{i}.txt"))
            .collect();
        let body = serde_json::json!({ "paths": paths }).to_string();
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let text = body_text(resp).await;
        assert!(
            text.contains(&MAX_BATCH_PATHS.to_string()),
            "the refusal must say what the limit is: {text}"
        );
    }

    /// The workspace chokepoint answers before any git runs: a standalone agent
    /// whose folder has no repository cannot stage anything.
    #[tokio::test]
    async fn stage_files_in_a_folder_with_no_repository_is_refused() {
        let (_tmp, app, _state) = router_with_session_and_state().await;
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/sa1/git/stage-files",
                r#"{"paths":["f.txt"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}
