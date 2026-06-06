//! axum router + the `/ws` handler bridging the browser to the engine actor.
//!
//! ## Route structure and the auth gate
//!
//! Routes split into OPEN and GATED groups. The split is a CONVENTION, not a
//! compile-time guarantee — nothing stops a route from being misplaced into the
//! open group — so add every new data route to the `gated` sub-router below to
//! keep it behind the gate:
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
    /// How often a live, authenticated `/ws` socket re-verifies that its user
    /// still exists (see [`handle_socket`]). The HTTP gate cannot revoke an
    /// ALREADY-upgraded socket, so this periodic recheck closes that gap. A
    /// constructor-injectable field so the revocation e2e can drive a short
    /// window instead of waiting the production cadence.
    pub ws_recheck_period: std::time::Duration,
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
    build_router(engine, auth, Router::new())
}

/// Production cadence for the live `/ws` user-existence recheck. Short enough
/// that a revoked user's open socket dies within a few seconds, long enough that
/// the per-socket `Vec` scan under a brief read lock is negligible overhead.
const WS_RECHECK_PERIOD: std::time::Duration = std::time::Duration::from_secs(4);

/// Shared router builder. `extra_gated` is merged INTO the gated sub-router
/// before the gate middleware is layered on, so any route it carries inherits
/// the session gate exactly as `/ws` does. Production callers pass an empty
/// router; a test injects a probe route to prove the gate covers arbitrary data
/// routes, not just `/ws` (see [`tests::gated_data_route_is_401_without_session`]).
fn build_router(engine: EngineHandle, auth: SharedAuth, extra_gated: Router<AppState>) -> Router {
    build_router_with_recheck(engine, auth, extra_gated, WS_RECHECK_PERIOD)
}

/// Like [`build_router`] but with an injectable `/ws` recheck period so the
/// revocation e2e can drive a short window instead of the production cadence.
pub fn build_router_with_recheck(
    engine: EngineHandle,
    auth: SharedAuth,
    extra_gated: Router<AppState>,
    ws_recheck_period: std::time::Duration,
) -> Router {
    let state = AppState {
        engine,
        auth,
        rate_limiter: RateLimiter::default(),
        ws_recheck_period,
    };

    // In-memory session store: sessions die with the server (documented v1
    // limitation — a restart forces re-login). HttpOnly and SameSite=Strict are
    // the tower-sessions defaults but we set them explicitly so the intent is
    // visible and a future default change can't silently weaken the cookie.
    //
    // TODO(step 7 TLS): the `MemoryStore` never sweeps EXPIRED sessions — an
    // expired record lingers in the map until the process exits, so a long-lived
    // server with churn slowly accumulates dead entries. Run
    // `tower_sessions::ExpiredDeletion` (a periodic sweep task) against the store
    // when the persistent/TLS-fronted deployment lands, so memory stays bounded.
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

    // GATED routes: the gate middleware runs before these. By convention, add
    // every new data route here so it inherits the session requirement — the
    // structure can't stop a route from being misplaced into the open group.
    let gated = Router::new()
        .route("/ws", get(ws_upgrade))
        .merge(extra_gated)
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
///
/// Session PRESENCE is not sufficient: the session's username must STILL exist
/// in the current [`auth::AuthState`]. An operator who removes a user (config
/// edit or TUI palette) then reloads config expects that user's live session to
/// stop working immediately, without a server restart. So when auth is on we
/// re-check the username against the current snapshot on every request (a `Vec`
/// scan under a brief read lock — no bcrypt; see [`auth::username_exists`]) and,
/// if it is gone, flush the now-orphaned session and reject with `401`.
async fn gate(
    State(state): State<AppState>,
    session: Session,
    request: Request,
    next: Next,
) -> Response {
    // Auth disabled: the user-existence re-check does not apply; every request
    // passes (today's UX).
    if !auth::is_enabled(&state.auth) {
        return next.run(request).await;
    }
    // Session presence is not enough: the named user must STILL exist in the
    // current snapshot (shared with `/api/me`; flushes an orphaned session
    // internally). `None` → unauthenticated.
    match auth::session_user_if_valid(&state.auth, &session).await {
        Some(_) => next.run(request).await,
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Whether a WebSocket upgrade passes the same-host Origin check (cross-site
/// WebSocket hijacking defense). `true` when the request carries no `Origin`
/// (non-browser clients — CLIs, tests, native apps — don't send one, and the
/// tradeoff is documented) or when the `Origin`'s `host[:port]` matches the
/// `Host` header. `false` for a present-but-mismatched `Origin`. Browsers always
/// send `Origin` for WS, so this only ever rejects a genuine cross-site attempt.
/// Applies whether or not auth is enabled.
// TODO(step 7 TLS): add a Host allowlist to defend against DNS rebinding. The
// same-origin check below trusts the request's own `Host` header, so it does not
// stop a rebinding attacker who points a controlled hostname at this server's IP
// (the browser then sends a matching Origin/Host pair). Once dux terminates TLS
// (or is fronted by an HTTPS proxy), pin the set of allowed Host values to the
// configured public hostname(s) and reject everything else here.
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
    session: Session,
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

    // When auth is on, the gate already validated this session's username before
    // the upgrade. Re-read it (same way the gate does) so the socket loop can
    // periodically re-verify the user still exists — the HTTP gate cannot revoke
    // an ALREADY-upgraded socket, so without this recheck a removed user's open
    // socket would keep streaming AND accepting commands until it closes on its
    // own. Auth off (or somehow no username) → no recheck (auth-off sockets are
    // unaffected by the recheck machinery).
    let recheck_user = if auth::is_enabled(&state.auth) {
        session
            .get::<String>(auth::SESSION_USER_KEY)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let auth = state.auth.clone();
    let recheck_period = state.ws_recheck_period;
    ws.on_upgrade(move |socket| {
        handle_socket(socket, state.engine, auth, recheck_user, recheck_period)
    })
    .into_response()
}

type SharedSink = Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>;

/// Drive one upgraded `/ws` connection.
///
/// `recheck_user` is `Some(username)` only when auth is enabled and the gate
/// validated a session at upgrade; it is `None` for auth-off sockets (no
/// recheck). When `Some`, a `recheck_period` interval re-verifies the user still
/// exists in the live auth snapshot — the HTTP gate cannot revoke an already-
/// upgraded socket, so this recheck closes the gap: on failure we send a Close
/// frame and break, which kills BOTH the ViewModel/status streaming and command
/// intake. Auth-off sockets pass `recheck_user = None` and are never rechecked.
async fn handle_socket(
    socket: WebSocket,
    engine: EngineHandle,
    auth: SharedAuth,
    recheck_user: Option<String>,
    recheck_period: std::time::Duration,
) {
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

    // Periodic user-existence recheck (only when this is an authenticated
    // socket). The HTTP gate cannot revoke an already-upgraded socket, so this
    // recheck closes the gap. The first tick fires immediately, so we burn it to
    // align subsequent ticks to the period.
    let mut recheck = tokio::time::interval(recheck_period);
    if recheck_user.is_some() {
        recheck.tick().await;
    }

    loop {
        let msg = tokio::select! {
            // Re-verify the session's user still exists. On failure, close the
            // socket — this stops streaming AND command intake. Disabled for
            // auth-off sockets (`recheck_user` is None).
            //
            // Revocation means: the gate is still ON, but THIS user is gone.
            // A whole-gate downgrade (the gate flipped OFF entirely — e.g. a
            // loopback operator removed the last user and reloaded) is a
            // LOOSENING, not a revocation: the operator deliberately turned auth
            // off, so the gate no longer protects anyone and there is nothing to
            // revoke. Closing the operator's own live socket there would be
            // wrong (transient, but a needless disconnect). So short-circuit
            // when the gate is no longer enabled — stop rechecking, keep
            // streaming — BEFORE testing whether the user still exists.
            _ = recheck.tick(), if recheck_user.is_some() => {
                if !auth::is_enabled(&auth) {
                    // Gate downgraded to OFF: this is a loosening, not a
                    // revocation. Keep this socket streaming.
                    continue;
                }
                let still_valid = recheck_user
                    .as_deref()
                    .map(|u| auth::username_exists(&auth, u))
                    .unwrap_or(false);
                if !still_valid {
                    // Best-effort Close frame, then break to tear down.
                    let mut guard = sink.lock().await;
                    let _ = guard.send(Message::Close(None)).await;
                    break;
                }
                continue;
            }
            next = stream.next() => match next {
                Some(Ok(msg)) => msg,
                _ => break,
            },
        };
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
    use tower::ServiceExt; // for `oneshot`

    /// Boot a minimal headless engine handle for routing-only tests. The handle
    /// just needs to exist — the gated request 401s before it ever reaches the
    /// engine.
    fn test_engine_handle(tmp: &std::path::Path) -> crate::engine_actor::EngineHandle {
        let paths = dux_core::config::DuxPaths {
            root: tmp.to_path_buf(),
            config_path: tmp.join("config.toml"),
            sessions_db_path: tmp.join("sessions.sqlite3"),
            worktrees_root: tmp.join("worktrees"),
            lock_path: tmp.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        handle
    }

    /// A representative data route added to the GATED sub-router must 401 without
    /// a session when auth is on — proving the gate covers arbitrary gated data
    /// routes, not only `/ws`. This is the reviewer's regression test: it injects
    /// a probe route through `build_router`'s test seam so a future data route
    /// placed in the gated group is provably protected.
    #[tokio::test]
    async fn gated_data_route_is_401_without_session() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());

        // Auth ON (one user) so the gate enforces a session.
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let auth = auth::shared_auth(&[format!("alice:{hash}")], false);

        // Inject a dummy gated data route through the test seam.
        let probe: Router<AppState> =
            Router::new().route("/api/_test_gated", get(|| async { "secret" }));
        let app = build_router(handle, auth, probe);

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/_test_gated")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "a gated data route must reject an unauthenticated request before reaching its handler"
        );
    }

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
