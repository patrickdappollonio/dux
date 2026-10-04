//! HTTP endpoints for mutating git operations: stage, unstage, discard, commit,
//! push, and pull. Project-scoped git actions (source-checkout refresh and
//! checkout-default) live in [`crate::project_actions`].
//!
//! These are request/response so the web UI gets real completion + errors and
//! can drive per-action loading state. After a mutation each handler invalidates
//! the changed-files cache, which emits a `session.changes` event on `/ws/events`
//! so subscribed clients refetch `GET /api/v1/sessions/:id/changes`.
//!
//! `refresh-changes` is the one route here that mutates nothing: it performs only
//! that post-mutation refresh, so a change dux did not make through one of these
//! routes can be picked up now instead of on the next poll. It shares
//! [`refresh_changed_files_now`] with every handler here, every handler in
//! [`crate::file_routes`], and the file-drop upload, so they cannot drift apart.
//!
//! Every handler runs git off the engine actor thread AND off the async reactor,
//! so a slow or locked repo never stalls other clients. File-path ops pre-validate
//! that the path is one git tracks in the worktree: `changed_files` only ever
//! returns worktree-relative paths inside the tree, so membership proves both, and
//! unlike a canonicalize check it accepts deleted files, which appear in status but
//! no longer exist on disk.
//!
//! Any client that can reach the address can commit, push and discard in every
//! worktree: that follows from the single-tenant trusted-access model, and the
//! Host allowlist and same-origin check are not authentication.

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

/// The discard body: the path, and what the user confirmed the row was:
/// `"file"`, or a folder row's wire kind (`"directory"`,
/// `"nested_repository"`). Absent from an older client: then a plain file is
/// discarded as it always was and a directory of any kind is refused, because
/// nothing said the user was looking at a folder. A `"directory"` also names
/// `files`, how many files its dialog said would go: the delete refuses to
/// take more, and a `"directory"` without it confirms nothing.
#[derive(Deserialize)]
struct DiscardOp {
    path: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    files: Option<usize>,
}

impl DiscardOp {
    fn confirmed(&self) -> Option<dux_core::git::ConfirmedEntry> {
        match self.kind.as_deref() {
            Some("file") => Some(dux_core::git::ConfirmedEntry::File),
            Some("directory") => self
                .files
                .map(|files| dux_core::git::ConfirmedEntry::Folder { files }),
            Some("nested_repository") => Some(dux_core::git::ConfirmedEntry::Repository),
            _ => None,
        }
    }
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
    /// For each path in `refused` that was refused for a reason of its own
    /// (a folder holding nothing but repositories, a worktree of this
    /// repository) rather than for having left the list, the sentence that
    /// says why. Omitted when there are none.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    reasons: std::collections::BTreeMap<String, String>,
    /// What a stage batch left out on purpose (zero for an unstage batch).
    #[serde(flatten)]
    left_out: LeftOut,
}

/// Maximum number of paths one batch may name. `changed_files` folds a wholly
/// untracked folder into one row, but a repository can still hold tens of
/// thousands of individually changed files; the cap keeps one request bounded
/// and is answered with a sentence rather than a bare status.
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

/// The git-mutation routes, path-keyed on the session id, which `id_within_bound`
/// validates and each handler resolves to a worktree at its top. Project-scoped git
/// actions live in [`crate::project_actions`].
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

/// Recompute a session's changed files now: the pair of calls every route that
/// changes a file makes afterwards. dux has no file watcher, so anything dux did
/// not do through one of them catches up only on the next poll.
///
/// Both halves are needed. The engine call refreshes the lists the engine itself
/// serves; the invalidate drops the REST cache entry, so the next GET recomputes
/// rather than re-serving the pre-edit snapshot.
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

/// Resolve the directory the EDITOR may root at for an agent.
///
/// The editor's own door onto the filesystem, gated like the git routes rather
/// than left to open its own hole. A directory that is gone has no tree to
/// browse, no file to open and nowhere to save, and the ENOENT each request
/// would otherwise return names no path and offers no way back. `409` rather
/// than `404`, because the agent exists and the route is real.
///
/// Deliberately NOT gated on the repository verdict: editing outside a
/// repository is a supported thing to do, so only the directory being gone is
/// refused here.
pub(crate) async fn resolve_editor_worktree(
    state: &AppState,
    session_id: String,
) -> Result<PathBuf, RouteRejection> {
    if let Some(reason) = state
        .engine
        .session_missing_directory_reason(session_id.clone())
        .await
    {
        return Err((StatusCode::CONFLICT, reason).into_response().into());
    }
    resolve_worktree(state, session_id).await
}

/// Resolve the directory a CHANGES-PANEL route may run git in: a managed worktree,
/// or a standalone agent's folder when that folder is itself a repository.
///
/// Folder-driven rather than agent-driven, so a standalone agent pointed at a
/// repository gets a real changes panel; when the folder is not one, the refusal
/// carries the folder's own sentence, never a git error about a repository nobody
/// named. `409` rather than `404`, because the agent exists and the route is real
/// and only the folder cannot answer, which is the shape a locked repository has.
pub(crate) async fn resolve_changes_worktree(
    state: &AppState,
    session_id: String,
) -> Result<PathBuf, RouteRejection> {
    resolve_git_directory(state, session_id, GitAsk::Read).await
}

/// The same directory for a route that WRITES, gated on the engine's mutation
/// predicate instead. A separate entry point because it is a separate question: a
/// read-only repository view would show files nobody may stage, and asking the read
/// question in a mutating handler is how that difference goes unnoticed.
pub(crate) async fn resolve_mutation_worktree(
    state: &AppState,
    session_id: String,
) -> Result<PathBuf, RouteRejection> {
    resolve_git_directory(state, session_id, GitAsk::Mutate).await
}

/// Register a write into `root` for as long as the returned guard lives, so a
/// removal of that worktree waits for it. Refused with a 409 that says why once
/// the worktree's removal has begun: nothing new may start in a folder that is
/// about to go, and nothing may bring it back.
pub(crate) fn hold_root_for_write(
    state: &AppState,
    root: &Path,
    kind: dux_core::worktree_ops::WorktreeOpKind,
    what: &str,
) -> Result<dux_core::worktree_ops::WorktreeOpGuard, RouteRejection> {
    state
        .engine
        .worktree_ops()
        .hold(root, kind)
        .map_err(|refused| {
            (StatusCode::CONFLICT, refused.sentence(what).to_string())
                .into_response()
                .into()
        })
}

/// Hold every path an editor operation TOUCHES, not the editor's root: the
/// file written, the folder made, both ends of a move, the entry deleted. A
/// terminal's editor is rooted where the terminal started (a home folder, a
/// repository root), which can CONTAIN an agent's worktree, so holding the root
/// says nothing about a write into that worktree. Each target is checked
/// through the registry's one containment test, so a target in or under a
/// folder being removed is refused, and a removal of a folder waits for every
/// operation touching anything inside it. The targets are the root joined with
/// the request's relative paths, as spelled; the containment and symlink
/// checks of the operation itself still run after this.
pub(crate) fn hold_targets_for_write(
    state: &AppState,
    root: &Path,
    targets: &[&str],
    kind: dux_core::worktree_ops::WorktreeOpKind,
    what: &str,
) -> Result<Vec<dux_core::worktree_ops::WorktreeOpGuard>, RouteRejection> {
    let mut holds = vec![hold_root_for_write(state, root, kind, what)?];
    for target in targets {
        holds.push(hold_root_for_write(state, &root.join(target), kind, what)?);
    }
    Ok(holds)
}

/// What an editor delete or move holds while it runs (see
/// [`guard_destructive_targets`]). Dropping it lets everything go.
pub(crate) struct DestructiveGuard {
    _root: dux_core::worktree_ops::WorktreeOpGuard,
    pub(crate) claims: Vec<dux_core::worktree_ops::DestructiveClaim>,
}

/// The first step of an editor delete or move, the one destructive protocol
/// every such operation follows: a hold on the editor's root, and a CLAIM on
/// every target (both ends of a move). From the claim on, nothing new can
/// start in or under a target, so nothing lands there between the occupancy
/// check and the operation, and only a claim lets the operation be cleared.
/// A target something is already running in is refused with what that is,
/// one already being removed or moved is refused too, and one with a removal
/// running inside it is waited for, bounded. Blocking: call it off the async
/// runtime.
pub(crate) fn guard_destructive_targets(
    state: &AppState,
    root: &Path,
    targets: &[&str],
    what: &str,
) -> Result<DestructiveGuard, RouteRejection> {
    let ops = state.engine.worktree_ops();
    let mut guard = DestructiveGuard {
        _root: hold_root_for_write(
            state,
            root,
            dux_core::worktree_ops::WorktreeOpKind::EditorWrite,
            what,
        )?,
        claims: Vec::new(),
    };
    let root_key = dux_core::worktree_ops::path_key(root);
    for target in targets {
        let path = root.join(target);
        // The root itself is never deleted or moved (the operation refuses
        // it), and it is already held above.
        if dux_core::worktree_ops::path_key(&path) == root_key {
            continue;
        }
        match ops.claim_for_destructive(&path) {
            Ok(claim) => guard.claims.push(claim),
            Err(reason) => {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "dux did not {what} {}: {reason}.",
                        dux_core::home_path::shorten_home(&path)
                    ),
                )
                    .into_response()
                    .into());
            }
        }
    }
    Ok(guard)
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
            .unwrap_or_else(|| "dux cannot work with git in this folder.".to_string()),
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
        // A path inside a folded folder is answered for by the folder's row,
        // once git confirms it is a change it lists there.
        Ok((staged, unstaged)) => {
            use dux_core::git::{ChangesSide, rows_answering};
            let asked = [p.clone()];
            let on = |files: &[dux_core::model::ChangedFile], side| {
                rows_answering(&wt, files, side, &asked).is_ok_and(|set| set.contains(&p))
            };
            on(&staged, ChangesSide::Staged) || on(&unstaged, ChangesSide::Unstaged)
        }
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
/// `action` names what was attempted, in a form that reads inside a sentence, and
/// PREFIXES git's own message rather than replacing it: git's text is what carries
/// a hook's report, a signing failure, an unmerged-files instruction or a held
/// `index.lock`, and pointing at `dux.log` instead strands a remote browser on a
/// machine its reader may not reach.
///
/// The server's worktree path is stripped by
/// [`dux_core::git::redact_worktree_path`], a tidiness measure and not a security
/// boundary, applied at the source for commit, push and pull too so all three read
/// the same way on both surfaces. The unredacted chain still goes to `dux.log`.
///
/// The ordinary refusals do not come through here: `git::commit_preflight` catches
/// the empty-message and nothing-staged cases and answers 400 in its own wording.
async fn run_git<F, T>(
    action: &'static str,
    worktree: &Path,
    hold: dux_core::worktree_ops::WorktreeOpGuard,
    op: F,
) -> Result<T, RouteRejection>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let worktree = worktree.to_path_buf();
    // The hold rides INTO the blocking task: a client that disconnects drops
    // this future, and the git work it started must still keep its worktree
    // registered until it has actually finished.
    match tokio::task::spawn_blocking(move || {
        let _hold = hold;
        op()
    })
    .await
    {
        Ok(Ok(value)) => Ok(value),
        // A refusal is a sentence the user can act on (look again, act on a
        // row inside), not a git failure.
        Ok(Err(e)) if e.downcast_ref::<dux_core::git::Refusal>().is_some() => Err((
            StatusCode::BAD_REQUEST,
            dux_core::git::redact_worktree_path(&e.to_string(), &worktree),
        )
            .into_response()
            .into()),
        // Something lives where the operation would delete: a conflict with
        // what is there, said in the sentence that names it.
        Ok(Err(e)) if e.downcast_ref::<dux_core::destructive::Refused>().is_some() => {
            Err((StatusCode::CONFLICT, e.to_string()).into_response().into())
        }
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
    let session_id = id.clone();
    let worktree = match resolve_mutation_worktree(&state, id).await {
        Ok(w) => w,
        Err(r) => return r.into_response(),
    };
    let hold = match hold_root_for_write(
        &state,
        &worktree,
        dux_core::worktree_ops::WorktreeOpKind::GitChange,
        "change files in it",
    ) {
        Ok(hold) => hold,
        Err(r) => return r.into_response(),
    };
    if let Err(r) = validate_changed_path(&worktree, &op.path).await {
        return r.into_response();
    }
    if let Some(sentence) = dux_core::git::stage_refusal(&worktree, &op.path) {
        return (StatusCode::BAD_REQUEST, sentence).into_response();
    }
    // A folder is staged without the repositories inside it, and the answer
    // says how many it left out, so the browser can say so.
    let wt = worktree.clone();
    let paths = vec![op.path];
    let report = match run_git(STAGE_ACTION, &worktree, hold, move || {
        dux_core::git::stage_with_report(&wt, &paths)
    })
    .await
    {
        Ok(report) => report,
        Err(r) => return r.into_response(),
    };
    refresh_changed_files_now(&state, session_id, &worktree);
    (StatusCode::OK, Json(LeftOut::from(report))).into_response()
}

/// What a stage left out of the index on purpose: the repositories inside a
/// staged folder, which `git add` would otherwise record as links.
#[derive(serde::Serialize, Default)]
struct LeftOut {
    left_out_repositories: usize,
    left_out_worktrees: usize,
}

impl From<dux_core::git::StageReport> for LeftOut {
    fn from(report: dux_core::git::StageReport) -> Self {
        Self {
            left_out_repositories: report.left_out_repositories,
            left_out_worktrees: report.left_out_worktrees,
        }
    }
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
    Json(op): Json<DiscardOp>,
) -> Response {
    if !id_within_bound(&id) {
        return unknown_session();
    }
    let session_id = id.clone();
    let worktree = match resolve_mutation_worktree(&state, id).await {
        Ok(w) => w,
        Err(r) => return r.into_response(),
    };
    let hold = match hold_root_for_write(
        &state,
        &worktree,
        dux_core::worktree_ops::WorktreeOpKind::GitChange,
        "change files in it",
    ) {
        Ok(hold) => hold,
        Err(r) => return r.into_response(),
    };
    // Discard is destructive (deletes untracked files / restores tracked ones),
    // so the tracked-vs-untracked distinction is derived SERVER-SIDE from live
    // git status, never trusted from the client. This also rejects staged files
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
    let confirmed = op.confirmed();
    let path = op.path;
    // A folder or a nested repository is deleted whole, so the discard
    // follows the one destructive protocol: claim it (off the runtime, since
    // a removal running inside it is waited for), ask the occupancy question
    // under the claim, and clear it right before the delete. A refusal is a
    // 409 naming what lives there.
    let target = worktree.join(&path);
    let claim = match confirmed {
        Some(dux_core::git::ConfirmedEntry::Folder { .. })
        | Some(dux_core::git::ConfirmedEntry::Repository) => {
            let ops = state.engine.worktree_ops().clone();
            let claimed = target.clone();
            match tokio::task::spawn_blocking(move || ops.claim_for_destructive(&claimed)).await {
                Ok(Ok(claim)) => Some(claim),
                Ok(Err(reason)) => {
                    return (
                        StatusCode::CONFLICT,
                        format!(
                            "dux did not delete {}: {reason}.",
                            dux_core::home_path::shorten_home(&target)
                        ),
                    )
                        .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("claim task failed: {e}"),
                    )
                        .into_response();
                }
            }
        }
        _ => None,
    };
    let Some(check) = state.engine.destructive_check(target.clone()).await else {
        return (StatusCode::SERVICE_UNAVAILABLE, "engine unavailable").into_response();
    };
    // A folder that is no longer what the user confirmed is a refusal they can
    // act on (look again), which `run_git` answers as one.
    let files_deleted = match run_git("discard the file's changes", &worktree, hold, move || {
        dux_core::git::discard_confirmed(&wt, &path, untracked, confirmed, || match &claim {
            Some(claim) => check.clear(&[claim], "delete"),
            None => Err(dux_core::destructive::Refused(format!(
                "dux did not delete {}: nothing confirmed it as a folder",
                dux_core::home_path::shorten_home(&target)
            ))),
        })
    })
    .await
    {
        Ok(files) => files,
        Err(r) => return r.into_response(),
    };
    refresh_changed_files_now(&state, session_id, &worktree);
    // How many files actually went, which the browser reports rather than
    // the count its dialog showed.
    (
        StatusCode::OK,
        Json(serde_json::json!({ "files_deleted": files_deleted })),
    )
        .into_response()
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

/// Stage or unstage a whole batch: one validating `git status` read, one git call,
/// one changed-files refresh.
///
/// The batch is PARTITIONED rather than refused whole, and validation is
/// section-scoped because the two verbs mean opposite things: a path that left its
/// section between the click and the request is reported in `refused` while the
/// rest proceed, and only an empty present set is a 400. The git call itself stays
/// whole, so a path vanishing between the status read and the call fails the batch
/// with git's own error rather than becoming another `refused` entry.
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
    let hold = match hold_root_for_write(
        &state,
        &worktree,
        dux_core::worktree_ops::WorktreeOpKind::GitChange,
        "change files in it",
    ) {
        Ok(hold) => hold,
        Err(r) => return r.into_response(),
    };

    let wt = worktree.clone();
    let requested = paths.clone();
    let partition = tokio::task::spawn_blocking(move || {
        dux_core::git::changed_files(&wt).and_then(|(staged, unstaged)| {
            let (live, side) = match section {
                Section::Staged => (&staged, dux_core::git::ChangesSide::Staged),
                Section::Unstaged => (&unstaged, dux_core::git::ChangesSide::Unstaged),
            };
            // One git call confirms every path inside a folded folder, so an
            // ignored or missing file there is refused rather than acted on.
            let answered = dux_core::git::rows_answering(&wt, live, side, &requested)?;
            let mut seen = std::collections::HashSet::new();
            let mut done = Vec::new();
            let mut refused = Vec::new();
            for path in requested {
                if !seen.insert(path.clone()) {
                    continue;
                }
                // A file inside a folded folder is in the section when the
                // folder is and git lists it there, which is how an expanded
                // row validates.
                if answered.contains(&path) {
                    done.push(path);
                } else {
                    refused.push(path);
                }
            }
            Ok((done, refused))
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
            no_selected_files_left_message(
                refused.len(),
                section,
                refused.first().map(String::as_str).unwrap_or_default(),
            ),
        )
            .into_response();
    }

    // A path git may not be asked to stage (a worktree of this repository, a
    // folder holding nothing but repositories) is refused on its own with its
    // sentence, and the rest of the batch goes ahead.
    let wt = worktree.clone();
    let batch = done.clone();
    let partition = match run_git(section.action(), &worktree, hold, move || match section {
        Section::Staged => {
            dux_core::git::unstage_files(&wt, &batch).map(|()| dux_core::git::StagePartition {
                staged: batch,
                ..Default::default()
            })
        }
        Section::Unstaged => dux_core::git::stage_partitioned(&wt, &batch),
    })
    .await
    {
        Ok(partition) => partition,
        Err(r) => return r.into_response(),
    };
    if partition.staged.is_empty() {
        // Every path left was refused for a reason of its own: say the first.
        let sentence = partition
            .refused
            .first()
            .map(|(_, sentence)| sentence.clone())
            .unwrap_or_default();
        return (
            StatusCode::BAD_REQUEST,
            dux_core::git::redact_worktree_path(&sentence, &worktree),
        )
            .into_response();
    }
    let mut refused = refused;
    let mut reasons = std::collections::BTreeMap::new();
    for (path, sentence) in partition.refused {
        refused.push(path.clone());
        reasons.insert(path, sentence);
    }
    refresh_changed_files_now(&state, session_id, &worktree);
    (
        StatusCode::OK,
        Json(BatchResult {
            done: partition.staged,
            refused,
            reasons,
            left_out: LeftOut::from(partition.report),
        }),
    )
        .into_response()
}

/// What the single-path stage route attempts, the words its failure uses.
const STAGE_ACTION: &str = "stage the file";

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
    let hold = match hold_root_for_write(
        &state,
        &worktree,
        dux_core::worktree_ops::WorktreeOpKind::GitChange,
        "change files in it",
    ) {
        Ok(hold) => hold,
        Err(r) => return r.into_response(),
    };
    if let Err(r) = validate_changed_path(&worktree, &path).await {
        return r.into_response();
    }
    let wt = worktree.clone();
    if let Err(r) = run_git(action, &worktree, hold, move || op(wt, path)).await {
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
    let hold = match hold_root_for_write(
        &state,
        &worktree,
        dux_core::worktree_ops::WorktreeOpKind::Commit,
        "commit in it",
    ) {
        Ok(hold) => hold,
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
    if let Err(r) = run_git("commit the staged changes", &worktree, hold, move || {
        dux_core::git::commit(&wt, &message).map(|_| ())
    })
    .await
    {
        return r.into_response();
    }
    refresh_changed_files_now(&state, session_id, &worktree);
    StatusCode::OK.into_response()
}

/// `POST /api/v1/sessions/:id/git/refresh-changes`. dux invalidates its cached
/// answer whenever dux changes a file, but cannot see one the user changed from a
/// terminal, so this is how a user says "look again" without waiting out the poll.
/// It changes nothing on disk and only forces the read every mutating handler does.
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

// push and pull trigger the engine command through `apply_wire` rather than running
// raw git, which would lose the in-flight dedup, the leading-branch resolution and
// the busy/done status. A 200 means accepted; the outcome reaches the originating
// client as a scoped `status` event.

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

/// The refusal when every file the browser named has left the section it was
/// staged or unstaged from, naming the first one so the user can see which
/// list went stale.
fn no_selected_files_left_message(refused: usize, section: Section, first: &str) -> String {
    let word = section.word();
    let subject = if refused == 1 {
        "the selected file is not".to_string()
    } else {
        format!("none of the {refused} selected files are")
    };
    format!(
        "{subject} in this worktree's {word} changes any more (starting with \"{first}\"). \
         Refresh the changes and try again."
    )
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
    async fn router_with_session_and_state()
    -> (dux_core::test_scratch::ScratchDir, Router, AppState) {
        let tmp = dux_core::test_scratch::ScratchDir::new();
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
                .create_session(&sample_session("s1", wt.to_string_lossy().as_ref()))
                .unwrap();
            // A standalone agent in a plain directory: no repository, so every
            // mutating route must be refused by the workspace chokepoint.
            let plain = root.join("plain");
            std::fs::create_dir_all(&plain).unwrap();
            store
                .create_session(&standalone_session("sa1", plain.to_string_lossy().as_ref()))
                .unwrap();
            // A standalone agent whose folder sits INSIDE another repository,
            // which git would happily answer for from the parent.
            let parent = root.join("parent");
            std::fs::create_dir_all(parent.join("inside/node_modules")).unwrap();
            run_git(&parent, &["init", "-q"]);
            std::fs::write(parent.join("inside/node_modules/a.js"), "a\n").unwrap();
            store
                .create_session(&standalone_session(
                    "sa2",
                    parent.join("inside").to_string_lossy().as_ref(),
                ))
                .unwrap();
            // A standalone agent whose folder a test deletes.
            let doomed = root.join("doomed");
            std::fs::create_dir_all(&doomed).unwrap();
            store
                .create_session(&standalone_session(
                    "sa3",
                    doomed.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let engine = crate::test_support::bootstrap_test_engine(&paths).unwrap();
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
        let ok = dux_core::test_git::fixture_git()
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
            slot_tab_id: format!("{id}-slot"),
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
    /// The failure is built by pointing the worktree's `.git` at a gitdir that
    /// does not exist: `git status` then names that gitdir by absolute path on
    /// stderr, and `changed_files` passes that text through, so the server's
    /// layout would reach a browser that may be on another machine entirely.
    ///
    /// The directory itself stays: a worktree that is gone is its own verdict,
    /// refused with a 409 before classify runs, and the engine's background
    /// pollers reach that verdict on their own schedule, so deleting it made
    /// the status this test reads depend on which got there first.
    #[tokio::test]
    async fn discard_strips_the_server_path_from_a_classify_refusal() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::remove_dir_all(worktree.join(".git")).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree.join("no-such-gitdir").display()),
        )
        .unwrap();

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
        let hold = dux_core::worktree_ops::WorktreeOps::new()
            .hold(&worktree, dux_core::worktree_ops::WorktreeOpKind::Commit)
            .unwrap();
        let err = super::run_git("commit the staged changes", &worktree, hold, move || {
            Err::<(), _>(anyhow::anyhow!("git commit failed: {stderr}"))
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

    /// A folded folder is one path on the wire and the git routes act on the
    /// whole of it: stage it, unstage it, and discard it (which deletes it).
    /// A file inside it is a real change too, and the section validation lets
    /// it through once git lists it there: both surfaces name such paths by
    /// expanding a folder.
    #[tokio::test]
    async fn a_folded_folder_is_staged_unstaged_and_discarded_whole() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        for index in 0..12 {
            let dir = worktree
                .join("node_modules")
                .join(format!("pkg{}", index / 4));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("f{index}.js")), "x\n").unwrap();
        }
        let lists = |worktree: PathBuf| async move {
            tokio::task::spawn_blocking(move || dux_core::git::changed_files(&worktree).unwrap())
                .await
                .unwrap()
        };
        // A folder row of `count` files, whatever its fingerprint.
        let folder =
            |file: &dux_core::model::ChangedFile| (file.is_expandable(), file.file_count());

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage",
                r#"{"path":"node_modules"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{}", body_text(resp).await);
        let (staged, unstaged) = lists(worktree.clone()).await;
        assert!(unstaged.is_empty(), "{unstaged:?}");
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].path, "node_modules");
        assert_eq!(folder(&staged[0]), (true, 12));

        // One file inside the staged folder, named by its path as an expanded
        // row would name it.
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/unstage-files",
                r#"{"paths":["node_modules/pkg0/f0.js"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{}", body_text(resp).await);
        // The folder is staged in part now, so it opens up, but only down the
        // path to the unstaged file: its sibling packages stay folded.
        let (staged, unstaged) = lists(worktree.clone()).await;
        let mut rows: Vec<(String, bool, usize)> = staged
            .iter()
            .map(|f| (f.path.clone(), f.is_folder(), f.file_count()))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            vec![
                ("node_modules/pkg0/f1.js".to_string(), false, 1),
                ("node_modules/pkg0/f2.js".to_string(), false, 1),
                ("node_modules/pkg0/f3.js".to_string(), false, 1),
                ("node_modules/pkg1".to_string(), true, 4),
                ("node_modules/pkg2".to_string(), true, 4),
            ]
        );
        assert!(unstaged.iter().any(|f| f.path == "node_modules/pkg0/f0.js"));

        // Staging that file again makes the folder whole, and one row again.
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                r#"{"paths":["node_modules/pkg0/f0.js"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{}", body_text(resp).await);
        let (staged, _) = lists(worktree.clone()).await;
        assert_eq!(staged.len(), 1);
        assert_eq!(folder(&staged[0]), (true, 12));

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/unstage",
                r#"{"path":"node_modules"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{}", body_text(resp).await);
        let (staged, unstaged) = lists(worktree.clone()).await;
        assert!(staged.is_empty(), "{staged:?}");
        assert_eq!(unstaged.len(), 1);
        assert_eq!(folder(&unstaged[0]), (true, 12));

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"node_modules","kind":"directory","files":12}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{}", body_text(resp).await);
        assert!(!worktree.join("node_modules").exists());
        let (staged, unstaged) = lists(worktree.clone()).await;
        assert!(staged.is_empty() && unstaged.is_empty());
    }

    /// A path that climbs out of a folded folder is not a file inside it. Each
    /// route refuses one before git or the filesystem sees it, and the worktree
    /// is exactly as it was: before this, `node_modules/..` as a discard emptied
    /// the worktree, `.git` included.
    #[tokio::test]
    async fn crafted_paths_through_a_folded_folder_are_refused_by_every_route() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("node_modules/pkg")).unwrap();
        std::fs::write(worktree.join("node_modules/pkg/a.js"), "a\n").unwrap();
        let crafted = [
            "node_modules/..",
            "node_modules/../f.txt",
            "node_modules/./pkg/a.js",
            "node_modules//pkg/a.js",
            "node_modules/pkg/../../f.txt",
            "node_modules/../.git",
            "/node_modules/pkg/a.js",
        ];
        for path in crafted {
            let one = serde_json::json!({ "path": path }).to_string();
            let many = serde_json::json!({ "paths": [path] }).to_string();
            for (route, body) in [
                ("discard", &one),
                ("stage", &one),
                ("unstage", &one),
                ("stage-files", &many),
                ("unstage-files", &many),
            ] {
                let resp = app
                    .clone()
                    .oneshot(json_req(
                        "POST",
                        &format!("/api/v1/sessions/s1/git/{route}"),
                        body,
                    ))
                    .await
                    .unwrap();
                assert!(
                    resp.status().is_client_error() || resp.status().is_server_error(),
                    "{route} with {path:?} must be refused, got {}",
                    resp.status()
                );
            }
        }
        assert_eq!(
            std::fs::read_to_string(worktree.join("f.txt")).unwrap(),
            "line1\n"
        );
        assert!(worktree.join(".git/HEAD").exists());
        assert!(worktree.join("node_modules/pkg/a.js").exists());
        let (staged, unstaged) =
            tokio::task::spawn_blocking(move || dux_core::git::changed_files(&worktree).unwrap())
                .await
                .unwrap();
        assert!(staged.is_empty(), "{staged:?}");
        assert_eq!(unstaged.len(), 1, "{unstaged:?}");
    }

    /// The discard names what the user confirmed. A folder that became a
    /// repository before the request landed is refused, and a repository is
    /// deleted only when the request names it as one.
    #[tokio::test]
    async fn a_discard_is_refused_when_the_folder_changed_kind() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("scratch")).unwrap();
        std::fs::write(worktree.join("scratch/a.js"), "a\n").unwrap();
        run_git(&worktree.join("scratch"), &["init", "-q"]);

        for body in [
            r#"{"path":"scratch","kind":"directory","files":1}"#,
            r#"{"path":"scratch"}"#,
        ] {
            let resp = app
                .clone()
                .oneshot(json_req("POST", "/api/v1/sessions/s1/git/discard", body))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
            assert!(worktree.join("scratch/.git").exists(), "{body}");
        }
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"scratch","kind":"directory","files":1}"#,
            ))
            .await
            .unwrap();
        assert!(body_text(resp).await.contains("changed since you looked"));

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"scratch","kind":"nested_repository"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!worktree.join("scratch").exists());
    }

    /// A folder holding nothing but a repository has nothing to stage, since
    /// the repository is left out: the route refuses it as a refusal (400,
    /// with the sentence), not as a git failure, and stages nothing.
    #[tokio::test]
    async fn staging_a_folder_that_holds_only_a_repository_is_refused() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("only/lib")).unwrap();
        std::fs::write(worktree.join("only/lib/own.txt"), "own\n").unwrap();
        run_git(&worktree.join("only/lib"), &["init", "-q"]);

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage",
                r#"{"path":"only"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let text = body_text(resp).await;
        assert!(
            text.contains("nothing in \"only/\" to stage: it holds only repositories of their own"),
            "{text}"
        );
        let (staged, _) = dux_core::git::changed_files(&worktree).unwrap();
        assert!(staged.is_empty(), "{staged:?}");
    }

    /// In a batch, a folder holding nothing but a repository is refused on its
    /// own, with its sentence, and the rest of the batch is staged; a batch of
    /// nothing else is a 400 carrying that sentence.
    #[tokio::test]
    async fn a_batch_stage_refuses_a_repositories_only_folder_and_stages_the_rest() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("only/lib")).unwrap();
        std::fs::write(worktree.join("only/lib/own.txt"), "own\n").unwrap();
        run_git(&worktree.join("only/lib"), &["init", "-q"]);
        std::fs::write(worktree.join("plain.txt"), "plain\n").unwrap();

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                r#"{"paths":["only"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(
            body_text(resp)
                .await
                .contains("nothing in \"only/\" to stage")
        );

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                r#"{"paths":["only","plain.txt"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert_eq!(json["done"], serde_json::json!(["plain.txt"]));
        assert_eq!(json["refused"], serde_json::json!(["only"]));
        assert!(
            json["reasons"]["only"]
                .as_str()
                .is_some_and(|reason| reason.contains("it holds only repositories of their own")),
            "{json}"
        );
        let (staged, _) = dux_core::git::changed_files(&worktree).unwrap();
        assert_eq!(
            staged.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["plain.txt"]
        );
    }

    // ── Folder children (`changes_routes`), tested here for the helpers ─────

    fn get_req(uri: &str) -> Request<axum::body::Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn children_uri(dir: &str, side: &str) -> String {
        let dir: String = url_escape(dir);
        format!("/api/v1/sessions/s1/changes/children?dir={dir}&side={side}")
    }

    /// Percent-encode a query value, so a test can send any byte it likes.
    fn url_escape(raw: &str) -> String {
        raw.bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (byte as char).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect()
    }

    fn child_paths(json: &serde_json::Value) -> Vec<String> {
        json["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|child| child["path"].as_str().unwrap().to_string())
            .collect()
    }

    /// A folded folder lists one level: its files as rows, its folders folded
    /// again with their counts, the same row shape the listing uses.
    #[tokio::test]
    async fn folder_children_lists_one_level_of_a_folded_folder() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("node_modules/pkg0")).unwrap();
        std::fs::write(worktree.join("node_modules/pkg0/a.js"), "a\n").unwrap();
        std::fs::write(worktree.join("node_modules/pkg0/b.js"), "b\n").unwrap();
        std::fs::write(worktree.join("node_modules/top.js"), "t\n").unwrap();

        let resp = app
            .clone()
            .oneshot(get_req(&children_uri("node_modules", "unstaged")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert_eq!(json["dir"], "node_modules");
        assert_eq!(json["side"], "unstaged");
        assert_eq!(
            child_paths(&json),
            ["node_modules/pkg0", "node_modules/top.js"]
        );
        assert_eq!(json["children"][0]["kind"], "directory");
        assert_eq!(json["children"][0]["file_count"], 2);

        // A folder inside an expanded one expands the same way.
        let resp = app
            .clone()
            .oneshot(get_req(&children_uri("node_modules/pkg0", "unstaged")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert_eq!(
            child_paths(&json),
            ["node_modules/pkg0/a.js", "node_modules/pkg0/b.js"]
        );
    }

    /// A folder staged whole lists what the index holds, on the staged side.
    #[tokio::test]
    async fn folder_children_lists_a_staged_folder_from_the_index() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("app")).unwrap();
        std::fs::write(worktree.join("app/a.rs"), "a\n").unwrap();
        run_git(&worktree, &["add", "app"]);

        let resp = app
            .clone()
            .oneshot(get_req(&children_uri("app", "staged")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert_eq!(child_paths(&json), ["app/a.rs"]);
        assert_eq!(json["children"][0]["status"], "A");

        // The same folder is not a row on the other side.
        let resp = app
            .clone()
            .oneshot(get_req(&children_uri("app", "unstaged")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// A folder list is validated against the changes list the service
    /// already holds for the agent, so expanding (or refreshing) any number of
    /// folders runs no full status of its own; only a cold cache reads one.
    #[tokio::test]
    async fn folder_children_validate_against_the_cached_listing() {
        let (tmp, app, state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("node_modules/pkg0/deep")).unwrap();
        std::fs::write(worktree.join("node_modules/pkg0/deep/a.js"), "a\n").unwrap();
        std::fs::write(worktree.join("node_modules/top.js"), "t\n").unwrap();

        // Cold: nothing cached yet, so the first request reads a listing.
        let resp = app
            .clone()
            .oneshot(get_req(&children_uri("node_modules", "unstaged")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(state.changes.fresh_validation_reads(), 1);

        // Warm: the pane's own read fills the cache, and every folder after it
        // is validated against that.
        let resp = app
            .clone()
            .oneshot(get_req("/api/v1/sessions/s1/changes"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        for dir in [
            "node_modules",
            "node_modules/pkg0",
            "node_modules/pkg0/deep",
        ] {
            let resp = app
                .clone()
                .oneshot(get_req(&children_uri(dir, "unstaged")))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{dir}");
        }
        assert_eq!(
            state.changes.fresh_validation_reads(),
            1,
            "no further full reads"
        );
    }

    /// The folder list is a changes read like any other, resolved from the
    /// agent's folder: a plain folder, one inside somebody else's repository
    /// (which git would answer for from the parent) and one that is gone all
    /// get the folder's own quiet sentence, never a listing and never a raw
    /// git failure.
    #[tokio::test]
    async fn folder_children_are_refused_where_the_changes_panel_is_quiet() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        std::fs::remove_dir_all(tmp.path().join("doomed")).unwrap();
        for (agent, dir) in [
            ("sa1", "node_modules"),
            ("sa2", "node_modules"),
            ("sa2", "inside/node_modules"),
            ("sa3", "node_modules"),
        ] {
            let resp = app
                .clone()
                .oneshot(get_req(&format!(
                    "/api/v1/sessions/{agent}/changes/children?dir={}&side=unstaged",
                    url_escape(dir)
                )))
                .await
                .unwrap();
            let status = resp.status();
            let body = body_text(resp).await;
            assert_eq!(status, StatusCode::CONFLICT, "{agent} {dir}: {body}");
            assert!(!body.contains("a.js"), "{agent} {dir}: {body}");
            assert!(
                !body.contains("Could not list the folder"),
                "{agent} {dir}: {body}"
            );
        }
    }

    /// A folder staged whole and then deleted from disk is still a staged row,
    /// and what the index holds for it is still listable: the staged side
    /// answers from the index, so it does not need the folder on disk.
    #[tokio::test]
    async fn folder_children_lists_a_staged_folder_that_is_gone_from_disk() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("build/sub")).unwrap();
        std::fs::write(worktree.join("build/a.o"), "a\n").unwrap();
        std::fs::write(worktree.join("build/sub/b.o"), "b\n").unwrap();
        run_git(&worktree, &["add", "build"]);
        std::fs::remove_dir_all(worktree.join("build")).unwrap();

        let resp = app
            .clone()
            .oneshot(get_req(&children_uri("build", "staged")))
            .await
            .unwrap();
        let status = resp.status();
        let body = body_text(resp).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(child_paths(&json), ["build/a.o", "build/sub"]);
    }

    /// A refusal names a file as a file, with no trailing slash.
    #[tokio::test]
    async fn folder_children_refusal_names_a_file_without_a_slash() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("node_modules")).unwrap();
        std::fs::write(worktree.join("node_modules/top.js"), "t\n").unwrap();
        std::fs::write(worktree.join("node_modules/b.js"), "b\n").unwrap();

        let resp = app
            .clone()
            .oneshot(get_req(&children_uri("node_modules/top.js", "unstaged")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_text(resp).await;
        assert!(body.contains("\"node_modules/top.js\""), "{body}");
        assert!(!body.contains("top.js/"), "{body}");
    }

    /// Only a folder the live listing shows (or one inside it that git lists
    /// something under) may be listed: a crafted path, a file, a tracked or
    /// missing folder, a path climbing out, one through a symlink, and a bad
    /// side are all refused, and nothing outside the worktree is ever read.
    #[tokio::test]
    async fn folder_children_refuses_anything_that_is_not_a_listed_folder() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("node_modules/pkg0")).unwrap();
        std::fs::write(worktree.join("node_modules/pkg0/a.js"), "a\n").unwrap();
        std::fs::write(worktree.join("node_modules/top.js"), "t\n").unwrap();
        std::fs::create_dir_all(tmp.path().join("outside")).unwrap();
        std::fs::write(tmp.path().join("outside/secret.txt"), "s\n").unwrap();
        std::os::unix::fs::symlink(
            tmp.path().join("outside"),
            worktree.join("node_modules/link"),
        )
        .unwrap();

        for (dir, side) in [
            ("", "unstaged"),
            ("/", "unstaged"),
            ("..", "unstaged"),
            ("../outside", "unstaged"),
            ("node_modules/..", "unstaged"),
            ("node_modules/../..", "unstaged"),
            ("node_modules/./pkg0", "unstaged"),
            ("node_modules//pkg0", "unstaged"),
            ("/etc", "unstaged"),
            ("node_modules/top.js", "unstaged"),
            ("node_modules/link", "unstaged"),
            ("node_modules/missing", "unstaged"),
            ("f.txt", "unstaged"),
            (".git", "unstaged"),
            ("node_modules", "sideways"),
        ] {
            let resp = app
                .clone()
                .oneshot(get_req(&children_uri(dir, side)))
                .await
                .unwrap();
            let status = resp.status();
            let body = body_text(resp).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{dir:?} {side}: {body}");
            assert!(!body.contains("secret.txt"), "{dir:?}: {body}");
        }

        let resp = app
            .clone()
            .oneshot(get_req(
                "/api/v1/sessions/nobody/changes/children?dir=node_modules&side=unstaged",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// A folder delete carries how many files the dialog said would go: more
    /// now is refused, fewer goes, and the answer says how many actually went.
    /// A folder confirmation without a count is no confirmation at all.
    #[tokio::test]
    async fn a_folder_discard_is_held_to_the_count_it_confirmed() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        for name in ["a", "b", "c", "d", "e"] {
            std::fs::create_dir_all(worktree.join("out")).unwrap();
            std::fs::write(worktree.join(format!("out/{name}.js")), "x\n").unwrap();
        }

        for body in [
            r#"{"path":"out","kind":"directory","files":3}"#,
            r#"{"path":"out","kind":"directory"}"#,
        ] {
            let resp = app
                .clone()
                .oneshot(json_req("POST", "/api/v1/sessions/s1/git/discard", body))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
            assert!(worktree.join("out/e.js").exists(), "{body}");
        }
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"out","kind":"directory","files":3}"#,
            ))
            .await
            .unwrap();
        assert!(body_text(resp).await.contains("it now holds 5 files"));

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"out","kind":"directory","files":9}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert_eq!(json["files_deleted"], 5);
        assert!(!worktree.join("out").exists());
    }

    /// A file row confirmed as a file, which a folder has since replaced, is
    /// refused and the folder is left alone; so is the same request from an
    /// older client that sends no kind at all.
    #[tokio::test]
    async fn a_discard_confirmed_for_a_file_is_refused_when_a_folder_took_its_place() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("x")).unwrap();
        std::fs::write(worktree.join("x/a.txt"), "a\n").unwrap();

        for body in [r#"{"path":"x","kind":"file"}"#, r#"{"path":"x"}"#] {
            let resp = app
                .clone()
                .oneshot(json_req("POST", "/api/v1/sessions/s1/git/discard", body))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
            assert!(worktree.join("x/a.txt").exists(), "{body}");
        }
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"x","kind":"file"}"#,
            ))
            .await
            .unwrap();
        assert!(body_text(resp).await.contains("changed since you looked"));

        std::fs::write(worktree.join("loose.txt"), "l\n").unwrap();
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"loose.txt","kind":"file"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!worktree.join("loose.txt").exists());
    }

    /// Staging a folder stages its files and leaves the repositories inside it
    /// out; both stage routes say how many they left out, so the browser can.
    #[tokio::test]
    async fn staging_a_folder_leaves_its_repositories_out_and_says_so() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join("vendor/lib")).unwrap();
        std::fs::write(worktree.join("vendor/x.js"), "x\n").unwrap();
        run_git(&worktree.join("vendor/lib"), &["init", "-q"]);
        std::fs::write(worktree.join("vendor/lib/own.txt"), "own\n").unwrap();
        run_git(&worktree.join("vendor/lib"), &["add", "own.txt"]);
        run_git(
            &worktree.join("vendor/lib"),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "own",
            ],
        );
        run_git(
            &worktree,
            &["worktree", "add", "-q", "-b", "side", "vendor/wt"],
        );

        for (route, body) in [
            ("stage", r#"{"path":"vendor"}"#),
            ("stage-files", r#"{"paths":["vendor"]}"#),
        ] {
            run_git(&worktree, &["reset", "-q"]);
            let resp = app
                .clone()
                .oneshot(json_req(
                    "POST",
                    &format!("/api/v1/sessions/s1/git/{route}"),
                    body,
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{route}");
            let parsed: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
            assert_eq!(parsed["left_out_repositories"], 1, "{route}: {parsed}");
            assert_eq!(parsed["left_out_worktrees"], 1, "{route}: {parsed}");
            let index = std::process::Command::new("git")
                .args([
                    "-C",
                    worktree.to_string_lossy().as_ref(),
                    "ls-files",
                    "--stage",
                ])
                .output()
                .unwrap();
            let index = String::from_utf8_lossy(&index.stdout);
            assert!(!index.contains("160000"), "{route}: {index}");
            assert!(index.contains("vendor/x.js"), "{route}: {index}");
        }
    }

    /// A worktree of this same repository placed inside the worktree is the
    /// worktree manager's: both stage routes refuse it with a sentence, as a
    /// refusal rather than a failure, and nothing reaches the index.
    #[tokio::test]
    async fn staging_a_linked_worktree_is_refused_by_both_routes() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        run_git(
            &worktree,
            &["worktree", "add", "-q", "-b", "side", "inner-wt"],
        );

        for (route, body) in [
            ("stage", r#"{"path":"inner-wt"}"#),
            ("stage-files", r#"{"paths":["inner-wt"]}"#),
        ] {
            let resp = app
                .clone()
                .oneshot(json_req(
                    "POST",
                    &format!("/api/v1/sessions/s1/git/{route}"),
                    body,
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{route}");
            assert!(
                body_text(resp).await.contains("worktree manager"),
                "{route}"
            );
        }
        let (staged, _) =
            tokio::task::spawn_blocking(move || dux_core::git::changed_files(&worktree).unwrap())
                .await
                .unwrap();
        assert!(staged.is_empty(), "{staged:?}");
    }

    /// A folder row answers only for what git lists inside it: an ignored file
    /// or a missing one is refused by every route, and the ignored file stays.
    #[tokio::test]
    async fn a_path_inside_a_folder_that_git_does_not_list_is_refused() {
        let (tmp, app, _state) = router_with_session_and_state().await;
        let worktree = tmp.path().join("wt");
        std::fs::write(worktree.join(".gitignore"), "*.env\n").unwrap();
        run_git(&worktree, &["add", ".gitignore"]);
        run_git(&worktree, &["commit", "-q", "-m", "ignore"]);
        std::fs::create_dir_all(worktree.join("build")).unwrap();
        std::fs::write(worktree.join("build/app.js"), "a\n").unwrap();
        std::fs::write(worktree.join("build/secret.env"), "SECRET=1\n").unwrap();

        for (route, body) in [
            ("discard", r#"{"path":"build/secret.env"}"#),
            ("discard", r#"{"path":"build/nope.js"}"#),
            ("stage", r#"{"path":"build/secret.env"}"#),
        ] {
            let resp = app
                .clone()
                .oneshot(json_req(
                    "POST",
                    &format!("/api/v1/sessions/s1/git/{route}"),
                    body,
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{route} {body}");
        }
        assert!(worktree.join("build/secret.env").exists());

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/stage-files",
                r#"{"paths":["build/app.js","build/nope.js","build/secret.env"]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert_eq!(parsed["done"], serde_json::json!(["build/app.js"]));
        assert_eq!(
            parsed["refused"],
            serde_json::json!(["build/nope.js", "build/secret.env"])
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

    #[test]
    fn the_stale_selection_refusal_counts_the_files() {
        assert_eq!(
            no_selected_files_left_message(1, Section::Staged, "a.rs"),
            "the selected file is not in this worktree's staged changes any more (starting \
             with \"a.rs\"). Refresh the changes and try again."
        );
        assert_eq!(
            no_selected_files_left_message(3, Section::Unstaged, "a.rs"),
            "none of the 3 selected files are in this worktree's unstaged changes any more \
             (starting with \"a.rs\"). Refresh the changes and try again."
        );
    }

    /// Review 6: "the same check also decides every destructive file
    /// operation: the changes pane's folder discard, nested-repo discard". The
    /// web changes pane's discard route deletes an untracked folder whole
    /// without asking the one occupancy question, so a standalone agent whose
    /// folder sits untracked inside the worktree loses its folder (dux "never
    /// creates, moves or removes a standalone agent's folder").
    #[tokio::test]
    async fn review6_web_discard_does_not_delete_a_standalone_agents_folder() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let root = tmp.path().to_path_buf();
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        run_git(&wt, &["init", "-q"]);
        run_git(&wt, &["config", "user.email", "t@example.com"]);
        run_git(&wt, &["config", "user.name", "t"]);
        std::fs::write(wt.join("f.txt"), "line1\n").unwrap();
        run_git(&wt, &["add", "f.txt"]);
        run_git(&wt, &["commit", "-q", "-m", "init"]);
        let scratch = wt.join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("notes.md"), "the standalone agent's work\n").unwrap();
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
                .create_session(&sample_session("s1", wt.to_string_lossy().as_ref()))
                .unwrap();
            store
                .create_session(&standalone_session(
                    "s-alone",
                    scratch.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let engine = crate::test_support::bootstrap_test_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        let app = crate::server::build_app(
            handle,
            Router::new(),
            crate::server::RouterParams::plain_http(),
        );
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/sessions/s1/git/discard",
                r#"{"path":"scratch","kind":"directory","files":1}"#,
            ))
            .await
            .unwrap();
        let status = resp.status();
        let text = body_text(resp).await;
        assert!(
            scratch.join("notes.md").exists(),
            "the web changes pane deleted standalone agent s-alone's folder {} \
             (answered {status}: {text})",
            scratch.display()
        );
        assert_eq!(status, StatusCode::CONFLICT, "{text}");
    }
}
