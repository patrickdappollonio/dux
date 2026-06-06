//! The engine runs on its own thread (the `Engine` is `!Send`). Async code talks to it
//! through `EngineHandle`: requests over a tokio unbounded mpsc (so the handle is
//! `Send + Sync` for use as axum state), the engine thread polling it with `try_recv` on
//! a tick (so it also drains worker events and refreshes the ViewModel watch); replies
//! over tokio oneshots; the latest ViewModel JSON over a tokio watch channel.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
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
    /// Snapshot the inputs needed to classify a project's managed worktrees:
    /// the project, the dux paths, and the current sessions. Instant clones off
    /// engine state; the git work (`list_worktrees` + classification) runs
    /// off-thread in the server handler (it shells to git), mirroring how
    /// `SessionWorktree` feeds the off-thread diff. `None` when the project id is
    /// unknown.
    ProjectWorktreeInputs(
        String,
        oneshot::Sender<
            Option<(
                dux_core::model::Project,
                dux_core::config::DuxPaths,
                Vec<dux_core::model::AgentSession>,
            )>,
        >,
    ),
    /// Gracefully wind down every running PTY (SIGTERM the children so CLIs can
    /// save state for a later resume), then stop the engine thread. Replies once
    /// the wind-down completes so the server can finish exiting.
    Shutdown(oneshot::Sender<()>),
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

/// Per-iteration control for [`run_engine_loop`]. Checked once at the top of
/// every outer loop iteration: `Continue` runs another tick, `Exit` stops the
/// loop and returns the engine to the caller. The in-process flip's status
/// screen drives this (via [`crate::serve_with_engine`]); the dedicated-thread
/// path always returns `Continue` (it exits only on the `Shutdown` request).
pub enum LoopControl {
    Continue,
    Exit,
}

/// The loop-side ends of the actor channels, owned by [`run_engine_loop`].
/// Split out from [`EngineHandle`] (the caller-facing ends) so both the
/// dedicated-thread path and the in-process flip can build the channels once
/// and run the same loop body.
pub(crate) struct ActorLoopEnds {
    req_rx: mpsc::UnboundedReceiver<EngineRequest>,
    vm_tx: watch::Sender<String>,
    status_tx: broadcast::Sender<WireStatus>,
    commit_msg_tx: broadcast::Sender<String>,
    /// Shared with the caller-facing [`EngineHandle`] and every PTY forwarder.
    /// The inline `Shutdown` request trips this so forwarders exit promptly even
    /// before the engine drop disconnects their channels.
    shutdown_flag: Arc<AtomicBool>,
    /// Live-reload hook for the login gate: the shared auth snapshot the server's
    /// router reads, paired with the `--disable-auth` flag captured at startup.
    /// When a config reload lands (`ApplyReloadedConfig`), the loop rebuilds the
    /// snapshot from the new `[auth]` users so credential changes take effect
    /// without a server restart. `None` when no gate is wired (e.g. tests that
    /// build channels directly).
    auth_reload: Option<(crate::auth::SharedAuth, bool)>,
}

/// Build the actor channels and split them into the caller-facing
/// [`EngineHandle`] and the loop-side [`ActorLoopEnds`]. Both server entry
/// points (the dedicated engine thread and the in-process flip) call this so
/// the channel topology is defined in exactly one place.
pub(crate) fn build_actor_channels(engine: &Engine) -> (EngineHandle, ActorLoopEnds) {
    build_actor_channels_with_auth(engine, None)
}

/// Like [`build_actor_channels`], but threads the live login-gate reload hook
/// into the loop ends so a config reload rebuilds the server's shared auth
/// snapshot from the new `[auth]` users.
pub(crate) fn build_actor_channels_with_auth(
    engine: &Engine,
    auth_reload: Option<(crate::auth::SharedAuth, bool)>,
) -> (EngineHandle, ActorLoopEnds) {
    let (req_tx, req_rx) = mpsc::unbounded_channel::<EngineRequest>();
    let (vm_tx, vm_rx) = watch::channel(view_model_json(engine));
    let (status_tx, _status_rx) = broadcast::channel::<WireStatus>(256);
    let (commit_msg_tx, _commit_msg_rx) = broadcast::channel::<String>(64);
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    (
        EngineHandle {
            req_tx,
            view_model_rx: vm_rx,
            status_tx: status_tx.clone(),
            commit_msg_tx: commit_msg_tx.clone(),
            shutdown_flag: Arc::clone(&shutdown_flag),
        },
        ActorLoopEnds {
            req_rx,
            vm_tx,
            status_tx,
            commit_msg_tx,
            shutdown_flag,
            auth_reload,
        },
    )
}

/// Spawn the four global background workers on `engine`. Both `App::run` (the
/// TUI) and every server entry point spawn these, and the in-process flip hands
/// the SAME engine — with these workers already running — to the other surface,
/// which calls this again. The spawn helpers are individually idempotent for
/// the long-lived pollers (see `dux_core::engine`), so a redundant call here is
/// safe: it will not start a second poller.
pub(crate) fn spawn_global_workers(engine: &mut Engine) {
    engine.spawn_changed_files_poller();
    engine.spawn_branch_sync_worker();
    engine.spawn_project_branch_status_checks();
    engine.spawn_gh_status_check();
}

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
    commit_msg_tx: broadcast::Sender<String>,
    /// Tripped when the server is tearing down (ReturnToTui, QuitProcess, or a
    /// `Shutdown` request). PTY forwarders poll it so their blocking
    /// `recv_timeout` loop exits promptly even when the engine — and therefore
    /// the std-mpsc `Sender` in the `PtyClient` reader thread — stays alive
    /// across the flip. Without this, a forwarder parked on a never-disconnecting
    /// channel would wedge the tokio blocking pool and hang the runtime teardown.
    shutdown_flag: Arc<AtomicBool>,
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

    /// The teardown flag PTY forwarders poll. Cloned into each forwarder so a
    /// blocking `recv_timeout` loop can break within one timeout window once the
    /// server starts winding down, even though the underlying `PtyClient`'s
    /// `Sender` outlives the flip (ReturnToTui keeps PTYs alive). The same flag
    /// is held loop-side ([`ActorLoopEnds`]) and by `serve_with_engine`, which
    /// trips it the instant the engine loop returns.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown_flag)
    }

    pub fn subscribe_view_model(&self) -> watch::Receiver<String> {
        self.view_model_rx.clone()
    }

    pub fn subscribe_status(&self) -> broadcast::Receiver<WireStatus> {
        self.status_tx.subscribe()
    }

    pub fn subscribe_commit_messages(&self) -> broadcast::Receiver<String> {
        self.commit_msg_tx.subscribe()
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

    /// Gracefully wind down the engine: SIGTERM the agent/terminal children so
    /// CLIs can save state for a later resume, then stop the engine thread.
    /// Errors are ignored — if the thread is already gone, shutdown has already
    /// happened.
    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self.req_tx.send(EngineRequest::Shutdown(tx)).is_ok() {
            let _ = rx.await;
        }
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

    /// Snapshot the inputs to classify a project's managed worktrees (project,
    /// paths, sessions). Instant — the git classification runs off-thread in the
    /// caller. `None` when the project id is unknown.
    #[allow(clippy::type_complexity)]
    pub async fn project_worktree_inputs(
        &self,
        project_id: String,
    ) -> Option<(
        dux_core::model::Project,
        dux_core::config::DuxPaths,
        Vec<dux_core::model::AgentSession>,
    )> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::ProjectWorktreeInputs(project_id, tx))
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }
}

/// Spawn the engine thread. Returns a handle and the thread's join handle.
///
/// This is the dedicated-thread server path (`dux server`): the engine lives on
/// its own std thread for its whole life. The channel setup, worker spawns, and
/// loop body are shared with the in-process flip ([`crate::serve_with_engine`])
/// via [`build_actor_channels`], [`spawn_global_workers`], and
/// [`run_engine_loop`] — this path simply runs the loop on a spawned thread and
/// drops the returned engine. The control closure always returns `Continue`, so
/// the loop's only exit is the inline `Shutdown` request, exactly as before.
pub fn spawn_engine_thread(mut engine: Engine) -> (EngineHandle, JoinHandle<()>) {
    let (handle, ends) = build_actor_channels(&engine);
    spawn_global_workers(&mut engine);

    let join = thread::spawn(move || {
        // The dedicated thread never asks the loop to exit; the loop stops only
        // on the `Shutdown` request handled inline. The returned engine is
        // dropped here (thread end), exactly as the previous implementation did.
        let _engine = run_engine_loop(engine, ends, || LoopControl::Continue);
    });

    (handle, join)
}

/// Like [`spawn_engine_thread`], but threads the login-gate reload hook into the
/// loop so a config reload rebuilds the server's shared auth snapshot. Used by
/// the `dux server` CLI path ([`crate::run_server`]).
pub fn spawn_engine_thread_with_auth(
    mut engine: Engine,
    auth_reload: (crate::auth::SharedAuth, bool),
) -> (EngineHandle, JoinHandle<()>) {
    let (handle, ends) = build_actor_channels_with_auth(&engine, Some(auth_reload));
    spawn_global_workers(&mut engine);

    let join = thread::spawn(move || {
        let _engine = run_engine_loop(engine, ends, || LoopControl::Continue);
    });

    (handle, join)
}

/// The shared engine request/drain loop. Runs on the CALLER's thread (a spawned
/// std thread for `dux server`, the main thread for the in-process flip) and
/// owns `engine` for the loop's duration, returning it on exit so the flip can
/// resume the TUI around the same live engine (PTYs intact).
///
/// `control` is consulted once at the top of each outer iteration: `Exit` stops
/// the loop and returns the engine WITHOUT shutting down any PTYs (the flip's
/// ReturnToTui path relies on this). The inline `Shutdown` request still stops
/// the loop too — it SIGTERMs the children first (the CLI/quit teardown).
pub(crate) fn run_engine_loop(
    mut engine: Engine,
    ends: ActorLoopEnds,
    mut control: impl FnMut() -> LoopControl,
) -> Engine {
    let ActorLoopEnds {
        mut req_rx,
        vm_tx,
        status_tx: thread_status_tx,
        commit_msg_tx: thread_commit_tx,
        shutdown_flag,
        auth_reload,
    } = ends;
    // Subscribes waiting for their provider to come up via the worker-event drain.
    let mut pending: Vec<PendingSubscribe> = Vec::new();
    loop {
        // Caller-driven exit (the flip's status screen asked to stop). Checked
        // before any work so an exit takes effect on the next tick. PTYs are
        // left untouched — teardown, if any, is the caller's responsibility.
        if matches!(control(), LoopControl::Exit) {
            break;
        }

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
            // The checkout-default-branch inspection (worker 1) just produced a
            // Known-default reaction: spawn the switch (worker 2). Its completion
            // posts NonDefaultBranchCheckoutCompleted, whose status flows through
            // the wire_statuses_from_reaction drain above on the next iteration.
            for status in engine.drive_checkout_followup(&reaction) {
                let _ = thread_status_tx.send(status);
            }
            // The add-project "Check Out & Add" switch (worker 2) just succeeded:
            // AddProjectAfterBranchCheckout drives the actual project add here
            // (the TUI does this in workers.rs). A switch FAILURE instead produced
            // an error Status, already surfaced by the wire_statuses drain above.
            for status in engine.drive_add_project_followup(&reaction) {
                let _ = thread_status_tx.send(status);
            }
            // A new-agent-from-PR lookup (gh pr view) just resolved: the TUI would
            // open a name prompt here, but the web already sent the name, so
            // OpenNewAgentPromptForPr drives the actual CreateAgentRequest::PullRequest
            // dispatch. A lookup FAILURE instead produced an error Status, already
            // surfaced by the wire_statuses drain above.
            for status in engine.drive_pr_lookup_followup(&reaction) {
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
                    format!("Saved to the database, but config.toml could not be updated: {e:#}"),
                ));
            }

            // A one-shot commit-message worker completed: push the generated
            // message to subscribed web clients, or surface a failure on the
            // status stream. Handled via `&reaction` so it coexists with the
            // borrows above and stays before the by-value consume below.
            match &reaction {
                EventReaction::CommitMessageGenerated(msg) => {
                    let _ = thread_commit_tx.send(msg.clone());
                }
                EventReaction::CommitMessageFailed(err) => {
                    let _ = thread_status_tx.send(WireStatus::new(
                        "error",
                        format!("Couldn't generate a commit message: {err}"),
                    ));
                }
                _ => {}
            }

            // A reload worker re-read config.toml; apply the new config to the
            // running engine. This consumes `reaction`, so it MUST be the last
            // use of it in the loop body (all `&reaction` borrows above end
            // first). `ApplyReloadedConfig` and `ProjectPersistenceOutcome` are
            // distinct variants, so consuming here never skips the project sync.
            if let EventReaction::ApplyReloadedConfig(config) = reaction {
                match engine.apply_reloaded_config(*config) {
                    Ok(()) => {
                        // Rebuild the login gate's shared snapshot from the
                        // freshly-applied `[auth]` users so credential changes
                        // (add/remove/change password via config or the TUI
                        // palette) take effect without a server restart. The
                        // `--disable-auth` flag captured at startup is preserved.
                        if let Some((shared, disable_auth)) = auth_reload.as_ref()
                            && let Ok(mut guard) = shared.write()
                        {
                            *guard = crate::auth::AuthState::build(
                                &engine.config.auth.users,
                                *disable_auth,
                            );
                        }
                        let _ = thread_status_tx.send(WireStatus::new(
                            "info",
                            "Configuration reloaded. New settings are active.",
                        ));
                    }
                    Err(e) => {
                        let _ = thread_status_tx.send(WireStatus::new(
                            "error",
                            format!("Config reload failed to apply: {e:#}"),
                        ));
                    }
                }
            }
        }

        // Consume each provider's received-data flag once per tick and stamp
        // the engine's activity map, so bytes that arrived this tick count
        // toward the `working` projection in the ViewModel refresh below. This
        // is the single poll site for the web surface (the TUI run loop is the
        // single poll site for the other surface; the two never run at once).
        engine.poll_pty_activity();

        // Refresh companion-terminal foreground commands so the ViewModel's
        // `foreground_cmd` tracks what's running. The engine throttles this by
        // wall-clock (~2s), so calling it every tick is cheap.
        engine.refresh_terminal_foregrounds();

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
                Ok(EngineRequest::Shutdown(reply)) => {
                    // Trip the teardown flag first so any PTY forwarders exit
                    // promptly (symmetry with the flip; harmless here since the
                    // engine drop will also disconnect their channels).
                    shutdown_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    // SIGTERM children, wait briefly for them to flush state,
                    // then mark agent sessions Detached. Handled here (not in
                    // `handle_request`) because it must stop the loop.
                    engine.shutdown_ptys(Duration::from_millis(1500));
                    let _ = reply.send(());
                    disconnected = true;
                    break;
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
    engine
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
        // Shutdown is handled inline in the loop (it must stop the thread).
        EngineRequest::Shutdown(_) => unreachable!("Shutdown handled in the loop"),
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
        EngineRequest::ProjectWorktreeInputs(project_id, reply) => {
            let inputs = engine
                .projects
                .iter()
                .find(|p| p.id == project_id)
                .cloned()
                .map(|project| (project, engine.paths.clone(), engine.sessions.clone()));
            let _ = reply.send(inputs);
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

    #[tokio::test]
    async fn shutdown_acks_and_stops_the_engine_thread() {
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
        let mut engine = bootstrap_engine(&paths).expect("bootstrap");
        // A `cat`-backed companion terminal that must be SIGTERMed on shutdown.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        engine
            .create_companion_terminal("s1")
            .expect("create companion terminal");
        let (handle, join) = spawn_engine_thread(engine);

        // Shutdown should ack within a reasonable window (grace is 1.5s).
        tokio::time::timeout(std::time::Duration::from_secs(5), handle.shutdown())
            .await
            .expect("shutdown acked");

        // The engine thread has stopped, so further requests fail.
        let res = handle
            .apply_wire(WireCommand::ToggleAgentAutoReopen {
                session_id: "s1".to_string(),
                enabled: true,
            })
            .await;
        assert!(res.is_err(), "requests should fail after shutdown");

        // The thread should have exited; join in a blocking task to avoid blocking
        // the async runtime.
        tokio::task::spawn_blocking(move || join.join())
            .await
            .expect("join task")
            .expect("engine thread joined");
    }
}
