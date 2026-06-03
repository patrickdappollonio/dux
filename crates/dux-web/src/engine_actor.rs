//! The engine runs on its own thread (the `Engine` is `!Send`). Async code talks to it
//! through `EngineHandle`: requests over a tokio unbounded mpsc (so the handle is
//! `Send + Sync` for use as axum state), the engine thread polling it with `try_recv` on
//! a tick (so it also drains worker events and refreshes the ViewModel watch); replies
//! over tokio oneshots; the latest ViewModel JSON over a tokio watch channel.

use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dux_core::engine::{
    Command, Engine, EventReaction, InFlightKey, ProjectPersistenceView, PrunedPtyKind,
};
use dux_core::pty::PtyClient;
use dux_core::wire::{WireCommand, WireCommandOutcome, WireStatus};
use dux_core::worker::AgentLaunchKind;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

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
    WritePty(String, Vec<u8>),
    ResizePty(String, u16, u16),
    /// Subscribe to an existing companion terminal (no launch; replies immediately).
    SubscribeTerminal(String, oneshot::Sender<Result<PtySubscription, String>>),
    /// Create a companion terminal for a session, replying `(terminal_id, label)`.
    CreateTerminal(String, oneshot::Sender<Result<(String, String), String>>),
    /// Resolve a session's worktree path (instant lookup; diff I/O happens
    /// off-thread in the server handler).
    SessionWorktree(String, oneshot::Sender<Option<String>>),
}

/// Resolve the live PTY for an id, which may name either an agent provider
/// (keyed by `session_id`) or a companion terminal (keyed by `terminal_id`).
/// This unifies the write/resize path so the same input/resize routing serves
/// both agents and terminals via whichever id the connection is subscribed to.
fn pty_for<'a>(engine: &'a Engine, id: &str) -> Option<&'a PtyClient> {
    engine
        .providers
        .get(id)
        .or_else(|| engine.companion_terminals.get(id).map(|t| &t.client))
}

const TICK: Duration = Duration::from_millis(50);

/// How long to wait for an agent provider to come up before failing a subscribe.
/// The launch runs in a background worker and the provider appears via the
/// worker-event drain, so the subscribe reply is deferred until then.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(20);

/// A subscribe that is waiting for its provider to be launched/resumed. The reply
/// is held until `engine.providers` contains the session (success) or the
/// deadline passes (timeout).
struct PendingSubscribe {
    session_id: String,
    reply: Option<oneshot::Sender<Result<PtySubscription, String>>>,
    deadline: Instant,
}

#[derive(Clone)]
pub struct EngineHandle {
    req_tx: mpsc::UnboundedSender<EngineRequest>,
    view_model_rx: watch::Receiver<String>,
    status_tx: broadcast::Sender<WireStatus>,
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

    pub fn subscribe_status(&self) -> broadcast::Receiver<WireStatus> {
        self.status_tx.subscribe()
    }

    pub async fn apply_wire(&self, command: WireCommand) -> Result<WireCommandOutcome, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::ApplyWire(command, tx))
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

    pub async fn subscribe_terminal(&self, terminal_id: String) -> Result<PtySubscription, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::SubscribeTerminal(terminal_id, tx))
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub async fn create_terminal(&self, session_id: String) -> Result<(String, String), String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::CreateTerminal(session_id, tx))
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub async fn session_worktree(&self, session_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SessionWorktree(session_id, tx))
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }
}

/// Spawn the engine thread. Returns a handle and the thread's join handle.
pub fn spawn_engine_thread(mut engine: Engine) -> (EngineHandle, JoinHandle<()>) {
    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<EngineRequest>();
    let (vm_tx, vm_rx) = watch::channel(view_model_json(&engine));
    let (status_tx, _status_rx) = broadcast::channel::<WireStatus>(256);
    let thread_status_tx = status_tx.clone();

    engine.spawn_changed_files_poller();
    engine.spawn_branch_sync_worker();
    engine.spawn_project_branch_status_checks();
    engine.spawn_gh_status_check();

    let handle = thread::spawn(move || {
        // Subscribes waiting for their provider to come up via the worker-event drain.
        let mut pending: Vec<PendingSubscribe> = Vec::new();
        loop {
            // Draining worker events may insert a launched provider (AgentLaunchReady)
            // into `engine.providers`, which resolves pending subscribes below.
            while let Ok(event) = engine.worker_rx.try_recv() {
                let reaction = engine.process_worker_event(event);
                for status in dux_core::wire::wire_statuses_from_reaction(&reaction) {
                    let _ = thread_status_tx.send(status);
                }
                for status in engine.drive_delete_followup(&reaction) {
                    let _ = thread_status_tx.send(status);
                }

                // A project mutation just updated SQLite + in-memory projects; mirror
                // it into the portable config.toml so a later TUI start doesn't clobber it.
                if let EventReaction::ProjectPersistenceOutcome(outcome) = &reaction
                    && !matches!(
                        outcome.view,
                        ProjectPersistenceView::PersistenceFailed { .. }
                    )
                    && let Err(e) = engine.persist_projects_to_config()
                {
                    let _ = thread_status_tx.send(WireStatus::new(
                        "error",
                        format!(
                            "Saved to the database, but config.toml could not be updated: {e:#}"
                        ),
                    ));
                }
            }

            // Reap agent/terminal PTYs whose child process exited so they stop
            // lingering in `providers`/`companion_terminals` and disappear from
            // the ViewModel, broadcasting a status for each so web clients learn.
            for pruned in engine.prune_exited_ptys() {
                let status = match pruned.kind {
                    PrunedPtyKind::Agent => {
                        WireStatus::new("warning", format!("Agent \"{}\" exited.", pruned.label))
                    }
                    PrunedPtyKind::Terminal => {
                        WireStatus::new("info", format!("Terminal \"{}\" closed.", pruned.label))
                    }
                };
                let _ = thread_status_tx.send(status);
            }

            // Resolve or expire pending subscribes now that providers may have appeared.
            let now = Instant::now();
            pending.retain_mut(|p| {
                if let Some(client) = engine.providers.get(&p.session_id) {
                    if let Some(reply) = p.reply.take() {
                        let _ = reply.send(Ok(client.subscribe_with_repaint()));
                    }
                    false
                } else if !engine.is_in_flight(&InFlightKey::AgentLaunch(p.session_id.clone())) {
                    // The launch worker finished but no provider came up: it failed.
                    // Fail fast with a clear message instead of waiting for the timeout;
                    // the specific error was already broadcast on the status stream.
                    if let Some(reply) = p.reply.take() {
                        let _ = reply.send(Err(format!(
                            "Agent failed to launch for session {}. Check dux.log for details.",
                            p.session_id
                        )));
                    }
                    false
                } else if now > p.deadline {
                    if let Some(reply) = p.reply.take() {
                        let _ = reply.send(Err("timed out launching agent".to_string()));
                    }
                    false
                } else {
                    true
                }
            });

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
                    Ok(EngineRequest::SubscribePty(session_id, reply)) => {
                        handle_subscribe(&mut engine, &mut pending, session_id, reply);
                    }
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
            status_tx,
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
        // SubscribePty is handled inline in the loop (it needs `&mut pending`).
        EngineRequest::SubscribePty(_, _) => unreachable!("SubscribePty handled in the loop"),
        EngineRequest::WritePty(id, bytes) => {
            if let Some(client) = pty_for(engine, &id) {
                let _ = client.write_bytes(&bytes);
            }
        }
        EngineRequest::ResizePty(id, rows, cols) => {
            if let Some(client) = pty_for(engine, &id) {
                let _ = client.resize(rows, cols);
            }
        }
        EngineRequest::SubscribeTerminal(terminal_id, reply) => {
            let res = match engine.companion_terminals.get(&terminal_id) {
                Some(terminal) => Ok(terminal.client.subscribe_with_repaint()),
                None => Err("unknown terminal".to_string()),
            };
            let _ = reply.send(res);
        }
        EngineRequest::CreateTerminal(session_id, reply) => {
            let res = engine
                .create_companion_terminal(&session_id)
                .map_err(|e| e.to_string());
            let _ = reply.send(res);
        }
        EngineRequest::SessionWorktree(session_id, reply) => {
            let worktree = engine
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .map(|s| s.worktree_path.clone());
            let _ = reply.send(worktree);
        }
    }
}

/// Handle a `SubscribePty` request. If the provider already exists, reply
/// immediately. Otherwise launch/resume the real agent provider and defer the
/// reply via a `PendingSubscribe` until the provider comes up (or times out).
fn handle_subscribe(
    engine: &mut Engine,
    pending: &mut Vec<PendingSubscribe>,
    session_id: String,
    reply: oneshot::Sender<Result<PtySubscription, String>>,
) {
    if let Some(client) = engine.providers.get(&session_id) {
        let _ = reply.send(Ok(client.subscribe_with_repaint()));
        return;
    }
    match launch_agent(engine, &session_id) {
        Ok(()) => pending.push(PendingSubscribe {
            session_id,
            reply: Some(reply),
            deadline: Instant::now() + LAUNCH_TIMEOUT,
        }),
        Err(e) => {
            let _ = reply.send(Err(e));
        }
    }
}

/// Launch (or resume) the real agent provider for `session_id` through the
/// engine's standard launch flow. The provider is NOT inserted here: the
/// dispatched launch runs in a background worker and the provider appears later
/// via the worker-event drain (`process_agent_launch_ready`), the same path the
/// TUI uses. The caller's `PendingSubscribe` waits for it.
fn launch_agent(engine: &mut Engine, session_id: &str) -> Result<(), String> {
    let session = engine
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .cloned()
        .ok_or_else(|| format!("unknown session {session_id}"))?;
    // A launch is already running for this session: just wait for it.
    if engine.is_in_flight(&InFlightKey::AgentLaunch(session_id.to_string())) {
        return Ok(());
    }
    let resume = engine.should_resume_session(&session);
    let request = engine.build_agent_launch_request(
        session,
        resume,
        (24, 80),
        AgentLaunchKind::Reconnect {
            status_message: "Attaching to agent".to_string(),
        },
    );
    engine
        .apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        })
        .map_err(|e| e.to_string())?;
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
