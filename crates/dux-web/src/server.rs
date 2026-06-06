//! axum router + the `/ws` handler bridging the browser to the engine actor.
//!
//! ## Route structure and the auth gate
//!
//! Routes split into OPEN and GATED groups so every future data route lands
//! behind the gate by construction:
//!
//! - OPEN: static assets, `/healthz`, `/api/login`, `/api/me`, `/api/logout`.
//!   The SPA must load (and call `/api/me`) to render the login screen, so these
//!   cannot require a session. `/api/logout` is idempotent, so it is open too.
//! - GATED: `/ws` (and any future data route added to the gated sub-router).
//!   When auth is on, the gate middleware rejects with `401` BEFORE the WS
//!   upgrade, so the browser sees a clean HTTP response rather than a socket that
//!   opens and immediately closes.
//!
//! The Origin check on `/ws` runs REGARDLESS of auth (cross-site WebSocket
//! hijacking defense): a browser attaches the page's `Origin`, and we only allow
//! same-host origins. Non-browser clients (no `Origin`) are allowed — documented
//! tradeoff: a CLI/test client is trusted to not be a hijacked browser tab.

use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{SinkExt, StreamExt};
use tower_sessions::cookie::SameSite;
use tower_sessions::cookie::time::Duration as CookieDuration;
use tower_sessions::{Expiry, MemoryStore, Session, SessionManagerLayer};

use crate::auth::{self, RateLimiter, SharedAuth};
use crate::engine_actor::EngineHandle;
use crate::protocol::{BranchWarningView, ClientMessage, ServerMessage};

#[derive(Clone)]
pub struct AppState {
    pub engine: EngineHandle,
    /// Parsed credentials + gate flag, shared so a config reload can rebuild it
    /// (see `engine_actor`). Read briefly by the login/me handlers and the gate.
    pub auth: SharedAuth,
    /// Per-IP login backoff. Shared (cheap `Arc` clone) so all login requests
    /// hit the same counters.
    pub rate_limiter: RateLimiter,
}

/// Build the router with the login gate OFF (no `[auth]` users). Kept as the
/// zero-argument entry the existing test harnesses and any no-auth caller use;
/// it delegates to [`router_with_auth`] with an empty, disabled [`AuthState`].
pub fn router(engine: EngineHandle) -> Router {
    router_with_auth(engine, auth::shared_auth(&[], false))
}

/// Build the axum router with an explicit shared auth snapshot.
///
/// `auth` carries the parsed credentials and the gate flag; when it reports the
/// gate disabled, the gate middleware passes everything through (today's UX). The
/// session layer is always installed (it is inert when no session is created), so
/// turning auth on via a config reload needs no router rebuild.
pub fn router_with_auth(engine: EngineHandle, auth: SharedAuth) -> Router {
    let state = AppState {
        engine,
        auth,
        rate_limiter: RateLimiter::default(),
    };

    // In-memory session store: sessions die with the server (documented v1
    // limitation — a restart forces re-login). HttpOnly and SameSite=Strict are
    // the tower-sessions defaults but we set them explicitly so the intent is
    // visible and a future default change can't silently weaken the cookie.
    let session_layer = SessionManagerLayer::new(MemoryStore::default())
        .with_name(auth::SESSION_COOKIE_NAME)
        .with_http_only(true)
        .with_same_site(SameSite::Strict)
        // TODO(step 7 TLS): set `.with_secure(true)` once dux terminates TLS (or
        // is always fronted by an HTTPS proxy). Until then a Secure cookie would
        // never be sent over the plain-HTTP loopback/dev deployment, locking
        // everyone out.
        .with_secure(false)
        .with_expiry(Expiry::OnInactivity(CookieDuration::days(
            auth::SESSION_INACTIVITY_DAYS,
        )));

    // GATED routes: the gate middleware runs before these. Future data routes go
    // here so they inherit the session requirement automatically.
    let gated = Router::new()
        .route("/ws", get(ws_upgrade))
        .route_layer(middleware::from_fn_with_state(state.clone(), gate));

    // OPEN routes: reachable without a session so the SPA can boot and log in.
    Router::new()
        .merge(gated)
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .fallback(crate::web_assets::static_handler)
        .layer(session_layer)
        .with_state(state)
}

/// Gate middleware for the protected sub-router. When auth is enabled, a valid
/// session is required; otherwise the request is rejected with `401` BEFORE the
/// WS upgrade. When auth is disabled, every request passes (today's UX).
async fn gate(
    State(state): State<AppState>,
    session: Session,
    request: Request,
    next: Next,
) -> Response {
    if !auth::is_enabled(&state.auth) {
        return next.run(request).await;
    }
    match session.get::<String>(auth::SESSION_USER_KEY).await {
        Ok(Some(_)) => next.run(request).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Whether a WebSocket upgrade passes the same-host Origin check (cross-site
/// WebSocket hijacking defense). `true` when the request carries no `Origin`
/// (non-browser clients — CLIs, tests, native apps — don't send one, and the
/// tradeoff is documented) or when the `Origin`'s `host[:port]` matches the
/// `Host` header. `false` for a present-but-mismatched `Origin`. Browsers always
/// send `Origin` for WS, so this only ever rejects a genuine cross-site attempt.
/// Applies whether or not auth is enabled.
fn same_origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        // No Origin: a non-browser client. Allowed (documented tradeoff).
        return true;
    };
    let origin = origin.to_str().ok().and_then(origin_host);
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| h.to_string());

    matches!((origin, host), (Some(o), Some(h)) if o == h)
}

/// Extract the `host[:port]` authority from an `Origin` header value
/// (`scheme://host[:port]`), so it can be compared against the `Host` header.
fn origin_host(origin: &str) -> Option<String> {
    let after_scheme = origin.split_once("://").map(|(_, rest)| rest)?;
    // Strip any path/query that shouldn't appear in an Origin but be defensive.
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Origin check runs even with auth off (CSWSH defense). On rejection we
    // return a 403 and never upgrade.
    if !same_origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin WebSocket upgrade rejected",
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state.engine))
        .into_response()
}

type SharedSink = Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>;

async fn handle_socket(socket: WebSocket, engine: EngineHandle) {
    let (sink, mut stream) = socket.split();
    let sink: SharedSink = Arc::new(tokio::sync::Mutex::new(sink));

    // Initial ViewModel.
    let _ = send_view_model(&sink, &engine.view_model_json()).await;

    // Forward ViewModel updates.
    {
        let sink = Arc::clone(&sink);
        let mut vm_rx = engine.subscribe_view_model();
        tokio::spawn(async move {
            while vm_rx.changed().await.is_ok() {
                let json = vm_rx.borrow_and_update().clone();
                if send_view_model(&sink, &json).await.is_err() {
                    break;
                }
            }
        });
    }

    // Forward engine status/lifecycle events (background completions, launch
    // failures, PTY exits) to this client.
    {
        let sink = Arc::clone(&sink);
        let mut status_rx = engine.subscribe_status();
        tokio::spawn(async move {
            loop {
                match status_rx.recv().await {
                    Ok(status) => {
                        let msg = ServerMessage::Status {
                            tone: status.tone,
                            message: status.message,
                        };
                        if send_json(&sink, &msg).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    // Forward AI-generated commit messages (produced by one-shot provider runs
    // after a `generate_commit_message` command) to this client.
    {
        let sink = Arc::clone(&sink);
        let mut commit_rx = engine.subscribe_commit_messages();
        tokio::spawn(async move {
            loop {
                match commit_rx.recv().await {
                    Ok(message) => {
                        if send_json(&sink, &ServerMessage::CommitMessage { message })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    let mut subscribed: Option<String> = None;
    // Exactly one live PTY forwarder per connection. Re-subscribing aborts the previous one so a
    // single PtyClient's output is never streamed to the same socket twice (which doubled echoed
    // input). React StrictMode double-mounts and session switching both trigger re-subscribes.
    let mut pty_forwarder: Option<tokio::task::JoinHandle<()>> = None;

    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Binary(bytes) => {
                if let Some(session_id) = &subscribed {
                    engine.write_pty(session_id.clone(), bytes.to_vec());
                }
            }
            Message::Text(text) => {
                let Ok(client_msg) = serde_json::from_str::<ClientMessage>(text.as_str()) else {
                    continue;
                };
                match client_msg {
                    ClientMessage::Command { command, args } => {
                        let envelope = serde_json::json!({ "command": command, "args": args });
                        match serde_json::from_value::<dux_core::wire::WireCommand>(envelope) {
                            Ok(wire) => {
                                let (status, error) = match engine.apply_wire(wire).await {
                                    Ok(outcome) => (outcome.status, None),
                                    Err(e) => (None, Some(e)),
                                };
                                let _ = send_json(
                                    &sink,
                                    &ServerMessage::CommandResult { status, error },
                                )
                                .await;
                            }
                            Err(e) => {
                                let _ = send_json(
                                    &sink,
                                    &ServerMessage::CommandResult {
                                        status: None,
                                        error: Some(format!("bad command: {e}")),
                                    },
                                )
                                .await;
                            }
                        }
                    }
                    ClientMessage::Subscribe { session_id } => {
                        match engine.subscribe_pty(session_id.clone()).await {
                            Ok((repaint, rx)) => {
                                // Stop the previous forwarder before streaming the new subscription.
                                if let Some(prev) = pty_forwarder.take() {
                                    prev.abort();
                                }
                                subscribed = Some(session_id.clone());
                                send_binary(&sink, repaint).await;
                                let _ = send_json(&sink, &ServerMessage::Subscribed { session_id })
                                    .await;
                                pty_forwarder = Some(spawn_pty_forwarder(
                                    Arc::clone(&sink),
                                    rx,
                                    engine.shutdown_flag(),
                                ));
                            }
                            Err(e) => {
                                let _ =
                                    send_json(&sink, &ServerMessage::Error { message: e }).await;
                            }
                        }
                    }
                    ClientMessage::Resize {
                        session_id,
                        rows,
                        cols,
                    } => {
                        engine.resize_pty(session_id, rows, cols);
                    }
                    ClientMessage::SubscribeTerminal { terminal_id } => {
                        match engine.subscribe_terminal(terminal_id.clone()).await {
                            Ok((repaint, rx)) => {
                                // Stop the previous forwarder before streaming the new subscription.
                                if let Some(prev) = pty_forwarder.take() {
                                    prev.abort();
                                }
                                subscribed = Some(terminal_id.clone());
                                send_binary(&sink, repaint).await;
                                let _ = send_json(
                                    &sink,
                                    &ServerMessage::Subscribed {
                                        session_id: terminal_id,
                                    },
                                )
                                .await;
                                pty_forwarder = Some(spawn_pty_forwarder(
                                    Arc::clone(&sink),
                                    rx,
                                    engine.shutdown_flag(),
                                ));
                            }
                            Err(e) => {
                                let _ =
                                    send_json(&sink, &ServerMessage::Error { message: e }).await;
                            }
                        }
                    }
                    ClientMessage::CreateTerminal { session_id } => {
                        match engine.create_terminal(session_id.clone()).await {
                            Ok((terminal_id, _label)) => {
                                let _ = send_json(
                                    &sink,
                                    &ServerMessage::TerminalCreated {
                                        session_id,
                                        terminal_id,
                                    },
                                )
                                .await;
                            }
                            Err(e) => {
                                let _ =
                                    send_json(&sink, &ServerMessage::Error { message: e }).await;
                            }
                        }
                    }
                    ClientMessage::GetDiff { session_id, path } => {
                        let (diff, error) = match engine.session_worktree(session_id.clone()).await
                        {
                            None => (None, Some("unknown session".to_string())),
                            Some(worktree) => {
                                let p = path.clone();
                                // git I/O off the engine thread AND off the async reactor.
                                match tokio::task::spawn_blocking(move || {
                                    dux_core::diff::file_diff(std::path::Path::new(&worktree), &p)
                                })
                                .await
                                {
                                    Ok(Ok(d)) => (Some(d), None),
                                    Ok(Err(e)) => (None, Some(e.to_string())),
                                    Err(e) => (None, Some(format!("diff task failed: {e}"))),
                                }
                            }
                        };
                        let _ = send_json(
                            &sink,
                            &ServerMessage::Diff {
                                session_id,
                                path,
                                diff,
                                error,
                            },
                        )
                        .await;
                    }
                    ClientMessage::BrowseDir { path } => {
                        let dir = path.unwrap_or_else(|| {
                            std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
                        });
                        // fs read off the reactor.
                        let result = tokio::task::spawn_blocking(move || {
                            let p = std::path::Path::new(&dir);
                            let entries = dux_core::project_browser::browser_entries(p)
                                .into_iter()
                                .map(|e| crate::protocol::DirEntryView {
                                    path: e.path.to_string_lossy().to_string(),
                                    label: e.label,
                                    is_git_repo: e.is_git_repo,
                                })
                                .collect::<Vec<_>>();
                            (dir, entries)
                        })
                        .await;
                        let msg = match result {
                            Ok((dir, entries)) => ServerMessage::DirEntries {
                                path: dir,
                                entries,
                                error: None,
                            },
                            Err(e) => ServerMessage::DirEntries {
                                path: String::new(),
                                entries: vec![],
                                error: Some(format!("browse failed: {e}")),
                            },
                        };
                        let _ = send_json(&sink, &msg).await;
                    }
                    ClientMessage::GenerateAgentName => {
                        // Pure, fast, and self-contained: answer directly without
                        // round-tripping through the engine thread.
                        let name = dux_core::git::docker_style_name();
                        let _ = send_json(&sink, &ServerMessage::AgentName { name }).await;
                    }
                    ClientMessage::ListProjectWorktrees { project_id } => {
                        // Resolve the project + classification inputs from the
                        // engine (an instant lookup), then classify off-thread:
                        // classification shells to git, so it must not run on the
                        // engine loop or the async reactor (the get_diff pattern).
                        let (entries, error) =
                            match engine.project_worktree_inputs(project_id.clone()).await {
                                None => (vec![], Some("unknown project".to_string())),
                                Some((project, paths, sessions)) => {
                                    match tokio::task::spawn_blocking(move || {
                                        classify_managed_worktrees(&project, &paths, &sessions)
                                    })
                                    .await
                                    {
                                        Ok(Ok(entries)) => (entries, None),
                                        Ok(Err(e)) => (vec![], Some(e)),
                                        Err(e) => {
                                            (vec![], Some(format!("worktree listing failed: {e}")))
                                        }
                                    }
                                }
                            };
                        let _ = send_json(
                            &sink,
                            &ServerMessage::ProjectWorktrees {
                                project_id,
                                entries,
                                error,
                            },
                        )
                        .await;
                    }
                    ClientMessage::InspectProjectPath { path } => {
                        // Pre-flight branch inspection mirroring the TUI's
                        // add_project: it runs `current_branch` +
                        // `branch_warning_kind` before showing the
                        // ConfirmNonDefaultBranch prompt. Both are bounded
                        // path-based git plumbing reads (no working-tree writes,
                        // no engine state — the path isn't a project yet), so
                        // run them directly off the reactor in spawn_blocking,
                        // following the browse_dir precedent.
                        let msg = inspect_project_path(path).await;
                        let _ = send_json(&sink, &msg).await;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Connection closed: stop the forwarder so it doesn't linger.
    if let Some(h) = pty_forwarder.take() {
        h.abort();
    }
}

/// How long the forwarder's blocking reader parks per `recv_timeout` before
/// re-checking `shutdown`. Bounds the worst-case time a forwarder lingers after
/// a teardown begins, so the tokio blocking pool never wedges runtime shutdown.
const FORWARDER_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Forward std-mpsc PTY bytes into the socket as binary frames, off the async runtime.
///
/// Returns the async pump task's [`JoinHandle`]. Aborting it drops `async_rx`, which makes the
/// blocking reader's `blocking_send` fail so the blocking task ends and drops its std `Receiver`;
/// the owning `PtyClient` then prunes that stale subscriber on its next read.
///
/// The blocking reader parks on a bounded `recv_timeout` rather than `recv` so it can also exit on
/// `shutdown`: the std-mpsc `Sender` lives in the `PtyClient` reader thread and, on a ReturnToTui
/// flip, the engine (and thus that `Sender`) stays alive, so `recv` would never return Disconnected
/// and would wedge the tokio blocking pool — hanging the runtime teardown. Polling `shutdown` every
/// `FORWARDER_POLL` lets the task exit within one window of any teardown even with the engine alive.
fn spawn_pty_forwarder(
    sink: SharedSink,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    let (tx, mut async_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    tokio::task::spawn_blocking(move || {
        loop {
            match rx.recv_timeout(FORWARDER_POLL) {
                Ok(chunk) => {
                    if tx.blocking_send(chunk).is_err() {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    tokio::spawn(async move {
        while let Some(chunk) = async_rx.recv().await {
            let mut guard = sink.lock().await;
            if guard.send(Message::Binary(chunk.into())).await.is_err() {
                break;
            }
        }
    })
}

async fn send_view_model(sink: &SharedSink, json: &str) -> Result<(), ()> {
    let value: serde_json::Value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    send_json(sink, &ServerMessage::ViewModel { data: value }).await
}

async fn send_json(sink: &SharedSink, msg: &ServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(msg).map_err(|_| ())?;
    let mut guard = sink.lock().await;
    guard.send(Message::Text(text.into())).await.map_err(|_| ())
}

async fn send_binary(sink: &SharedSink, bytes: Vec<u8>) {
    let mut guard = sink.lock().await;
    let _ = guard.send(Message::Binary(bytes.into())).await;
}

/// Classify a project's git worktrees and project the MANAGED ones (under dux's
/// worktrees root) into wire-safe entries. External worktrees and the project
/// checkout are excluded — they are not part of the managed-adoption flow (the
/// TUI offers external worktrees through its separate fork path). Each managed
/// entry is marked adoptable when it has no live agent; otherwise the reason
/// ("Already has an agent.") is surfaced so the client can show it disabled.
///
/// Runs in `spawn_blocking`: `list_worktrees` shells to git. Returns a
/// user-facing error string when the git listing fails.
fn classify_managed_worktrees(
    project: &dux_core::model::Project,
    paths: &dux_core::config::DuxPaths,
    sessions: &[dux_core::model::AgentSession],
) -> Result<Vec<crate::protocol::ProjectWorktreeEntryView>, String> {
    let worktrees = dux_core::git::list_worktrees(std::path::Path::new(&project.path))
        .map_err(|e| format!("{e:#}"))?;
    let entries =
        dux_core::project_browser::classify_project_worktrees(project, paths, sessions, worktrees)
            .into_iter()
            .filter(|entry| entry.is_managed_by_dux && !entry.is_project_checkout)
            .map(|entry| crate::protocol::ProjectWorktreeEntryView {
                worktree_path: entry.path.to_string_lossy().to_string(),
                branch_name: entry.branch_name,
                adoptable: entry.is_selectable,
                reason: if entry.is_selectable {
                    None
                } else {
                    Some("Already has an agent.".to_string())
                },
            })
            .collect();
    Ok(entries)
}

/// Pre-flight branch inspection for a candidate project path, mirroring the
/// TUI's `add_project`: it runs `current_branch` then `branch_warning_kind`
/// before deciding whether to show the non-default-branch warning. Both are
/// bounded git plumbing reads with no working-tree writes, so this runs off the
/// async reactor in `spawn_blocking` (the `browse_dir` precedent). `branch_warning_kind`
/// is a pure path-based read, so no engine state is needed — the path isn't a
/// registered project yet.
async fn inspect_project_path(path: String) -> ServerMessage {
    let echo = path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let repo = std::path::Path::new(&path);
        let branch = dux_core::git::current_branch(repo).map_err(|e| format!("{e:#}"))?;
        let warning = dux_core::git::branch_warning_kind(repo, &branch).map(|kind| match kind {
            dux_core::worker::BranchWarningKind::Known { default_branch } => {
                BranchWarningView::Known { default_branch }
            }
            dux_core::worker::BranchWarningKind::Heuristic => BranchWarningView::Heuristic,
        });
        Ok::<_, String>((branch, warning))
    })
    .await;
    match result {
        Ok(Ok((branch, warning))) => ServerMessage::ProjectPathInspection {
            path: echo,
            current_branch: Some(branch),
            warning,
            error: None,
        },
        Ok(Err(e)) => ServerMessage::ProjectPathInspection {
            path: echo,
            current_branch: None,
            warning: None,
            error: Some(e),
        },
        Err(e) => ServerMessage::ProjectPathInspection {
            path: echo,
            current_branch: None,
            warning: None,
            error: Some(format!("inspection task failed: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &std::path::Path, branch: &str) {
        run_git(dir, &["init", "-b", branch]);
        run_git(dir, &["config", "user.name", "test"]);
        run_git(dir, &["config", "user.email", "t@t"]);
        run_git(dir, &["commit", "--allow-empty", "-m", "init"]);
    }

    #[tokio::test]
    async fn inspect_project_path_on_default_branch_has_no_warning() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path(), "main");

        let msg = inspect_project_path(repo.path().to_string_lossy().into_owned()).await;
        match msg {
            ServerMessage::ProjectPathInspection {
                current_branch,
                warning,
                error,
                ..
            } => {
                assert_eq!(error, None);
                assert_eq!(current_branch.as_deref(), Some("main"));
                assert_eq!(warning, None);
            }
            other => panic!("expected ProjectPathInspection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inspect_project_path_heuristic_when_no_origin_head() {
        // `git init` repos lack refs/remotes/origin/HEAD, so a non-main/master
        // branch yields the Heuristic warning.
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path(), "develop");

        let msg = inspect_project_path(repo.path().to_string_lossy().into_owned()).await;
        match msg {
            ServerMessage::ProjectPathInspection {
                current_branch,
                warning,
                error,
                ..
            } => {
                assert_eq!(error, None);
                assert_eq!(current_branch.as_deref(), Some("develop"));
                assert_eq!(warning, Some(BranchWarningView::Heuristic));
            }
            other => panic!("expected ProjectPathInspection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inspect_project_path_known_default_when_origin_head_resolves() {
        // A clone gets refs/remotes/origin/HEAD pointing at the origin default,
        // so checking out a different branch yields the Known warning naming it.
        let origin = tempfile::tempdir().unwrap();
        init_repo(origin.path(), "main");

        let clone_dir = tempfile::tempdir().unwrap();
        let clone_path = clone_dir.path().join("work");
        run_git(
            clone_dir.path(),
            &[
                "clone",
                origin.path().to_string_lossy().as_ref(),
                clone_path.to_string_lossy().as_ref(),
            ],
        );
        run_git(&clone_path, &["switch", "-c", "feature/x"]);

        let msg = inspect_project_path(clone_path.to_string_lossy().into_owned()).await;
        match msg {
            ServerMessage::ProjectPathInspection {
                current_branch,
                warning,
                error,
                ..
            } => {
                assert_eq!(error, None);
                assert_eq!(current_branch.as_deref(), Some("feature/x"));
                assert_eq!(
                    warning,
                    Some(BranchWarningView::Known {
                        default_branch: "main".to_string(),
                    })
                );
            }
            other => panic!("expected ProjectPathInspection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inspect_project_path_non_repo_reports_error() {
        let dir = tempfile::tempdir().unwrap();
        let msg = inspect_project_path(dir.path().to_string_lossy().into_owned()).await;
        match msg {
            ServerMessage::ProjectPathInspection {
                current_branch,
                warning,
                error,
                ..
            } => {
                assert!(error.is_some(), "expected an error for a non-repo path");
                assert_eq!(current_branch, None);
                assert_eq!(warning, None);
            }
            other => panic!("expected ProjectPathInspection, got {other:?}"),
        }
    }
}
