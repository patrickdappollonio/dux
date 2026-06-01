//! The engine runs on its own thread (the `Engine` is `!Send`). Async code talks to it
//! through `EngineHandle`: requests over a tokio unbounded mpsc (so the handle is
//! `Send + Sync` for use as axum state), the engine thread polling it with `try_recv` on
//! a tick (so it also drains worker events and refreshes the ViewModel watch); replies
//! over tokio oneshots; the latest ViewModel JSON over a tokio watch channel.

use std::thread::{self, JoinHandle};
use std::time::Duration;

use dux_core::engine::Engine;
use dux_core::wire::{WireCommand, WireCommandOutcome};
use tokio::sync::{mpsc, oneshot, watch};

/// A PTY subscription: an initial repaint snapshot plus the live byte stream the caller
/// forwards. (PTY bytes never travel through the request channel.)
pub type PtySubscription = (Vec<u8>, std::sync::mpsc::Receiver<Vec<u8>>);

/// One unit of work for the engine thread.
pub enum EngineRequest {
    ApplyWire(
        WireCommand,
        oneshot::Sender<Result<WireCommandOutcome, String>>,
    ),
    SubscribePty(String, oneshot::Sender<Result<PtySubscription, String>>),
    EnsureDemoPty(String, oneshot::Sender<Result<(), String>>),
    WritePty(String, Vec<u8>),
    ResizePty(String, u16, u16),
}

const TICK: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub struct EngineHandle {
    req_tx: mpsc::UnboundedSender<EngineRequest>,
    view_model_rx: watch::Receiver<String>,
}

// Axum state must be `Send + Sync`; prove the handle satisfies that here so a future
// regression (e.g. swapping a channel type) fails at compile time, not at the axum
// router boundary.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EngineHandle>();
};

impl EngineHandle {
    pub fn view_model_json(&self) -> String {
        self.view_model_rx.borrow().clone()
    }

    pub fn subscribe_view_model(&self) -> watch::Receiver<String> {
        self.view_model_rx.clone()
    }

    pub async fn apply_wire(&self, command: WireCommand) -> Result<WireCommandOutcome, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::ApplyWire(command, tx))
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub async fn ensure_demo_pty(&self, session_id: String) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::EnsureDemoPty(session_id, tx))
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub async fn subscribe_pty(&self, session_id: String) -> Result<PtySubscription, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::SubscribePty(session_id, tx))
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub fn write_pty(&self, session_id: String, bytes: Vec<u8>) {
        let _ = self.req_tx.send(EngineRequest::WritePty(session_id, bytes));
    }

    pub fn resize_pty(&self, session_id: String, rows: u16, cols: u16) {
        let _ = self
            .req_tx
            .send(EngineRequest::ResizePty(session_id, rows, cols));
    }
}

/// Spawn the engine thread. Returns a handle and the thread's join handle.
pub fn spawn_engine_thread(mut engine: Engine) -> (EngineHandle, JoinHandle<()>) {
    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<EngineRequest>();
    let (vm_tx, vm_rx) = watch::channel(view_model_json(&engine));

    engine.spawn_changed_files_poller();
    engine.spawn_branch_sync_worker();
    engine.spawn_project_branch_status_checks();
    engine.spawn_gh_status_check();

    let handle = thread::spawn(move || {
        loop {
            while let Ok(event) = engine.worker_rx.try_recv() {
                let _ = engine.process_worker_event(event);
            }
            // Only notify ViewModel subscribers when the projection actually changed,
            // so idle ticks don't wake every WS client ~20x/second.
            let json = view_model_json(&engine);
            vm_tx.send_if_modified(|current| {
                if *current != json {
                    *current = json;
                    true
                } else {
                    false
                }
            });

            let mut disconnected = false;
            loop {
                match req_rx.try_recv() {
                    Ok(req) => handle_request(&mut engine, req),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if disconnected {
                break;
            }
            thread::sleep(TICK);
        }
    });

    (
        EngineHandle {
            req_tx,
            view_model_rx: vm_rx,
        },
        handle,
    )
}

fn view_model_json(engine: &Engine) -> String {
    serde_json::to_string(&engine.view_model()).unwrap_or_else(|_| "{}".to_string())
}

fn handle_request(engine: &mut Engine, req: EngineRequest) {
    match req {
        EngineRequest::ApplyWire(cmd, reply) => {
            let res = engine.apply_wire(cmd).map_err(|e| e.to_string());
            let _ = reply.send(res);
        }
        EngineRequest::EnsureDemoPty(session_id, reply) => {
            let _ = reply.send(ensure_demo_pty(engine, &session_id));
        }
        EngineRequest::SubscribePty(session_id, reply) => {
            let res = match engine.providers.get(&session_id) {
                Some(client) => Ok(client.subscribe_with_repaint()),
                None => Err(format!("no running provider for session {session_id}")),
            };
            let _ = reply.send(res);
        }
        EngineRequest::WritePty(session_id, bytes) => {
            if let Some(client) = engine.providers.get(&session_id) {
                let _ = client.write_bytes(&bytes);
            }
        }
        EngineRequest::ResizePty(session_id, rows, cols) => {
            if let Some(client) = engine.providers.get(&session_id) {
                let _ = client.resize(rows, cols);
            }
        }
    }
}

/// Spawn a plain PTY (the configured shell) for `session_id` if none is running, so PTY
/// streaming can be demonstrated without the full agent-launch flow. Real agent launch
/// over the web is a later plan.
fn ensure_demo_pty(engine: &mut Engine, session_id: &str) -> Result<(), String> {
    if engine.providers.contains_key(session_id) {
        return Ok(());
    }
    let session = engine
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| format!("unknown session {session_id}"))?;
    let worktree = std::path::PathBuf::from(&session.worktree_path);
    let cwd = if worktree.is_dir() {
        worktree
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?
    };
    let command = engine.config.terminal.command.clone();
    let args = engine.config.terminal.args.clone();
    let scrollback = engine.config.ui.agent_scrollback_lines;
    let client = dux_core::pty::PtyClient::spawn(&command, &args, &cwd, 24, 80, scrollback)
        .map_err(|e| e.to_string())?;
    engine.providers.insert(session_id.to_string(), client);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::bootstrap_engine;
    use dux_core::config::DuxPaths;

    fn temp_paths() -> (tempfile::TempDir, DuxPaths) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        (tmp, paths)
    }

    fn sample_session(
        id: &str,
        project_id: &str,
        branch: &str,
        worktree: &str,
    ) -> dux_core::model::AgentSession {
        let now = chrono::Utc::now();
        dux_core::model::AgentSession {
            id: id.to_string(),
            project_id: project_id.to_string(),
            project_path: None,
            provider: dux_core::model::ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: branch.to_string(),
            worktree_path: worktree.to_string(),
            title: Some(format!("{id}-title")),
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: false,
            status: dux_core::model::SessionStatus::Detached,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn apply_wire_toggle_reflects_in_view_model() {
        let (_tmp, paths) = temp_paths();
        {
            let store = dux_core::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .upsert_session(&sample_session(
                    "s1",
                    "p1",
                    "feat",
                    paths.root.to_string_lossy().as_ref(),
                ))
                .unwrap();
        }
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let (handle, _join) = spawn_engine_thread(engine);

        let outcome = handle
            .apply_wire(WireCommand::ToggleAgentAutoReopen {
                session_id: "s1".to_string(),
                enabled: true,
            })
            .await
            .expect("apply");
        assert!(outcome.status.is_some());

        let mut rx = handle.subscribe_view_model();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let json = rx.borrow_and_update().clone();
            if json.contains("\"id\":\"s1\"") && json.contains("\"auto_reopen_enabled\":true") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "view model never reflected toggle: {json}"
            );
            let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.changed()).await;
        }
    }
}
