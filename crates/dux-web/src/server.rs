//! axum router + the `/ws` handler bridging the browser to the engine actor.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};

use crate::engine_actor::EngineHandle;
use crate::protocol::{ClientMessage, ServerMessage};

#[derive(Clone)]
pub struct AppState {
    pub engine: EngineHandle,
}

/// Build the axum router serving the embedded web UI, a health check, and the `/ws` endpoint.
pub fn router(engine: EngineHandle) -> Router {
    let state = AppState { engine };
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/ws", get(ws_upgrade))
        .fallback(crate::web_assets::static_handler)
        .with_state(state)
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state.engine))
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
