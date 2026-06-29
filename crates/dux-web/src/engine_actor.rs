//! The engine runs on its own thread (the `Engine` is `!Send`). Async code talks to it
//! through `EngineHandle`: requests over a BOUNDED tokio mpsc (so the handle is
//! `Send + Sync` for use as axum state, and a misbehaving/flooding client cannot grow
//! the queue without limit — see [`REQ_CHANNEL_CAPACITY`]), the engine thread polling it
//! with `try_recv` on a tick (so it also drains worker events and fires the
//! coarse spine-change/status/commit signals); replies over tokio oneshots.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dux_core::engine::{
    Command, Engine, EventReaction, InFlightKey, ProjectPersistenceView, PrunedPtyKind,
};
use dux_core::pty::PtyClient;
use dux_core::statusline::{
    Generation, KeyedStatusController, KeyedWireStatus, StatusScope, StatusTone,
};
use dux_core::wire::{WireCommand, WireCommandOutcome, WireStatus};
use dux_core::worker::AgentLaunchKind;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

/// A PTY subscription: an initial repaint snapshot plus the live byte stream the caller
/// forwards. (PTY bytes never travel through the request channel.)
pub type PtySubscription = (Vec<u8>, std::sync::mpsc::Receiver<Vec<u8>>);

/// Which half of the projects/sessions spine changed since the last tick. The
/// engine loop fingerprints the projected spine each tick and fires the matching
/// variant; the web layer's forwarder turns it into a coarse `projects.changed` /
/// `sessions.changed` event so subscribed clients refetch `/api/v1/spine` (or the
/// thin per-resource read).
///
/// A single coarse signal per side is intentional for Phase 3: the sessions side
/// also covers session lifecycle/status, the `working` hysteresis flag, and the
/// per-session terminal list (they all live in the sessions/sidebar projection).
/// The spec's finer `session.status` / `session.working` / `terminals.changed`
/// split is an optional later optimization and is deliberately NOT implemented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpineChange {
    Projects,
    Sessions,
}

/// One unit of work for the engine thread.
pub enum EngineRequest {
    ApplyWire(
        WireCommand,
        oneshot::Sender<Result<WireCommandOutcome, String>>,
        /// Audience for any statuses this command mints. The actor sets
        /// `engine.current_origin` to this for the duration of `apply_wire` and
        /// resets it to [`StatusScope::All`] after, so a web operation's toasts
        /// reach only the originating connection. `All` is the broadcast default.
        StatusScope,
    ),
    /// A status from a non-engine producer (the ACME certificate-lifecycle task)
    /// to broadcast through the shared status controller so it auto-clears and
    /// reaches every client, exactly like engine-originated statuses.
    EmitStatus(WireStatus),
    SubscribePty(String, oneshot::Sender<Result<PtySubscription, String>>),
    WritePty(String, Vec<u8>),
    ResizePty(String, u16, u16),
    /// Subscribe to an existing companion terminal (no launch; replies immediately).
    SubscribeTerminal(String, oneshot::Sender<Result<PtySubscription, String>>),
    /// Create a companion terminal for a session, replying `(terminal_id, label)`.
    CreateTerminal(String, oneshot::Sender<Result<(String, String), String>>),
    /// Resolve the owning session id of a companion terminal (instant lookup), or
    /// `None` when the terminal id is unknown. Lets the nested PTY socket and the
    /// terminal REST routes enforce that a `:tid` belongs to its path `:id` before
    /// subscribing to or deleting it (the legacy `SubscribeTerminal`/`DeleteTerminal`
    /// path looks terminals up by id alone and does not check session ownership).
    TerminalSession(String, oneshot::Sender<Option<String>>),
    /// Resolve a session's worktree path (instant lookup; diff I/O happens
    /// off-thread in the server handler).
    SessionWorktree(String, oneshot::Sender<Option<String>>),
    /// Snapshot the build-/config-static bootstrap projection (providers, macros,
    /// palette commands, welcome tips, version, `ui.*` flags, gh availability,
    /// global env) served by `GET /api/v1/bootstrap`. Instant clone off engine
    /// state; refetched by the client on a `config.changed` event.
    Bootstrap(oneshot::Sender<dux_core::viewmodel::BootstrapView>),
    /// Snapshot the projects/sessions/sidebar spine served by the thin
    /// per-resource reads (`/api/v1/projects`, `/api/v1/sessions`). Instant clone
    /// off engine state; refetched by the client on a `projects.changed` /
    /// `sessions.changed` event. The hot whole-spine read (`GET /api/v1/spine`)
    /// instead uses [`EngineRequest::SpineJson`] (the loop's cached serialization).
    Spine(oneshot::Sender<dux_core::viewmodel::SpineView>),
    /// The pre-serialized whole-spine JSON for `GET /api/v1/spine`, served from the
    /// loop's cache (rebuilt only when the spine actually changes) instead of
    /// re-projecting + re-serializing on every client request. Handled inline in
    /// the loop because the cache is loop-local state.
    SpineJson(oneshot::Sender<String>),
    /// Project ONLY the requested session for `GET /api/v1/sessions/:id` instead of
    /// building the whole spine to find one session. `None` when the id is unknown
    /// (the handler returns 404).
    Session(
        String,
        oneshot::Sender<Option<dux_core::viewmodel::SessionView>>,
    ),
    /// Resolve the session id produced by a create op (the opaque id returned in
    /// `WireCommandOutcome.created_op_id`). Lets the REST create handler poll for
    /// ITS exact new session instead of a racy set-difference. `None` while the
    /// create is still in flight or the entry has expired.
    CreatedSessionForOp(String, oneshot::Sender<Option<String>>),
    /// Bump and return the next monotonic changed-files revision for a session
    /// (one actor round-trip over the engine's single SQLite connection). The
    /// `ChangesService` calls this at each detected change; the counter is
    /// persisted, so it never resets across restarts.
    NextChangesRev(String, oneshot::Sender<u64>),
    /// The configured preferred editor name (`config.editor.default`, e.g.
    /// "cursor"/"vscode"/"zed"). Instant clone; the detect + launch I/O for the
    /// "open in editor" action runs off-thread in the server handler.
    EditorDefault(oneshot::Sender<String>),
    /// Ask the engine to recompute the changed-files lists for a worktree (after
    /// an HTTP git mutation ran the git op off-thread). Fire-and-forget: the
    /// engine spawns its off-thread refresh worker, whose result flows back
    /// through the normal `ChangedFilesReady` path; the refreshed lists are then
    /// served by the REST changed-files read.
    RefreshChangedFiles(String),
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
    /// Resolve a session's startup-command-log context: `(dux paths, project_id)`
    /// for the session, so a GET handler can list/read its startup-command logs
    /// off-thread. `None` when the session id is unknown. Instant clone off engine
    /// state, mirroring [`EngineRequest::ProjectWorktreeInputs`].
    SessionStartupLogContext(
        String,
        oneshot::Sender<Option<(dux_core::config::DuxPaths, String)>>,
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

/// Run the spine fingerprint/cache check every Nth tick rather than every tick,
/// so idle serialization cost drops ~5x (one projection+serialize per ~250ms
/// instead of per 50ms). Time-based transitions — notably the `working` idle flip
/// when an agent stops streaming — are still detected within this window, and
/// every real mutation still fires its coarse event within ~250ms. A per-mutation
/// version counter would let the check run only on actual changes, but that is
/// far more invasive and drift-prone; this throttle is the safe mitigation.
const SPINE_CHECK_TICK_INTERVAL: u64 = 5;

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
    req_rx: mpsc::Receiver<EngineRequest>,
    status_tx: broadcast::Sender<WireStatus>,
    status_clear_tx: broadcast::Sender<Option<String>>,
    status_snapshot_tx: watch::Sender<Vec<KeyedWireStatus>>,
    /// Fires `()` once per successful config reload so the web layer can emit a
    /// `config.changed` event on its event bus (clients then refetch
    /// `/api/v1/bootstrap`). Broadcast — the web forwarder is the only listener,
    /// but a broadcast keeps the send a cheap fire-and-forget with no receiver.
    config_reload_tx: broadcast::Sender<()>,
    /// Fires a [`SpineChange`] whenever the projected projects-portion or
    /// sessions+sidebar-portion of the spine changes, so the web layer emits a
    /// coarse `projects.changed` / `sessions.changed` event (clients then refetch
    /// `/api/v1/spine`). Broadcast — the web forwarder is the only listener, but a
    /// broadcast keeps the send a cheap fire-and-forget with no receiver.
    spine_change_tx: broadcast::Sender<SpineChange>,
    /// Shared with the caller-facing [`EngineHandle`] and every PTY forwarder.
    /// The inline `Shutdown` request trips this so forwarders exit promptly even
    /// before the engine drop disconnects their channels.
    shutdown_flag: Arc<AtomicBool>,
    /// Live-reload hook for the login gate: the shared auth snapshot the server's
    /// router reads, the `--disable-auth` flag captured at startup, and whether
    /// every live listener is HOST-ONLY (genuine loopback). When a config reload
    /// lands (`ApplyReloadedConfig`), the loop rebuilds the snapshot from the new
    /// `[auth]` users so credential changes take effect without a server restart;
    /// the `host_only` flag lets the rebuild REFUSE a gate downgrade on a
    /// reachable bind (see `AuthState::rebuild`). `None` when no gate is wired
    /// (e.g. tests that build channels directly).
    auth_reload: Option<AuthReloadContext>,
}

/// The login-gate live-reload context threaded into the engine loop: the shared
/// snapshot, the startup `--disable-auth` flag, and whether every live listener
/// is HOST-ONLY loopback (which decides whether a reload may downgrade the gate
/// to open).
///
/// NOTE the deliberate distinction from the startup bind gate's "local"
/// classification: that gate treats a Tailscale bind as local (it does not need
/// `--insecure-allow-remote`), but `host_only` is the stricter DOWNGRADE rule —
/// it is `true` only when every listener is genuine loopback, so a Tailscale
/// bind is `host_only = false` and a running gate cannot silently open over a
/// shared tailnet. See `AuthState::rebuild` for the full rationale.
///
/// `pub` because the external `dux server` entry points and the auth integration
/// tests construct it to wire `spawn_engine_thread_with_auth`.
pub struct AuthReloadContext {
    pub shared: crate::auth::SharedAuth,
    pub disable_auth: bool,
    pub host_only: bool,
    /// The `dux server` terminal console. A config reload (and its drift warning)
    /// is echoed here so an operator watching the terminal sees it in the
    /// vite-style output, not just the WS status broadcast. A [`Console::noop`]
    /// for the flip/tests emits nothing; callers set this field directly (the CLI
    /// serve paths pass a real stdout console, the flip/tests pass a noop one).
    pub console: crate::console::Console,
}

/// True when a config reload changed any `[server]`/`[server.acme]` setting that
/// only takes effect at startup — listeners and the TLS acceptor are bound once,
/// and reload-config never rebinds them. The engine actor calls this on every
/// reload (before the config swap) so it can warn the user that a restart is
/// needed for these specific changes; a reload that only touched, say, `[ui]`
/// theme settings leaves every compared field equal and triggers no warning.
///
/// Compared fields mirror what the resolver consumes to bind: the LOCAL MODE
/// `port`, the `tailscale_enabled` toggle, the FULL WEB MODE `listen_addrs`, and
/// the entire `[server.acme]` section (any of its fields shifts the bound ports,
/// the issued domains, or staging-vs-production). `max_websocket_connections` is
/// also startup-bound: the `/ws` connection-cap semaphore is built ONCE in
/// `build_app` and never resized on reload, so changing the cap needs a restart
/// just like the listeners. `bind` is intentionally absent: it is deprecated and
/// migrated into `port`/`listen_addrs` on load, so a change to it surfaces through
/// those fields. `insecure_allow_remote` is a gate input, not a bound value, so it
/// cannot drift a live listener.
fn server_rebind_settings_changed(
    prev: &dux_core::config::ServerConfig,
    next: &dux_core::config::ServerConfig,
) -> bool {
    let a = &prev.acme;
    let b = &next.acme;
    prev.port != next.port
        || prev.tailscale_enabled != next.tailscale_enabled
        || prev.listen_addrs != next.listen_addrs
        || prev.max_websocket_connections != next.max_websocket_connections
        || a.enabled != b.enabled
        || a.domains != b.domains
        || a.email != b.email
        || a.http_port != b.http_port
        || a.https_port != b.https_port
        || a.production != b.production
        || a.cache_dir != b.cache_dir
}

/// Extract the reloaded `Config` from a reload follow-up reaction, consuming it.
///
/// The engine returns `ApplyReloadedConfig` bare in the common case, but folds it
/// into a `Multi` (alongside the deferred saves' status reactions) when
/// config-mutating commands were deferred during the reload. The actor must
/// handle BOTH so the auth-gate rebuild and server-restart warning always fire.
/// Returns `None` for any reaction that is not (and does not wrap) an
/// `ApplyReloadedConfig`.
fn take_apply_reloaded_config(reaction: EventReaction) -> Option<Box<dux_core::config::Config>> {
    match reaction {
        EventReaction::ApplyReloadedConfig(config) => Some(config),
        EventReaction::Multi(reactions) => {
            reactions.into_iter().find_map(take_apply_reloaded_config)
        }
        _ => None,
    }
}

/// Bound on the engine request channel. A burst buffer, not a steady-state
/// queue: the engine drains the WHOLE channel every `TICK` (50ms), so under
/// normal use it holds only a handful of in-flight requests. The cap exists so a
/// flooding or buggy client cannot grow the queue without limit (the old channel
/// was unbounded). Reply-bearing sends apply backpressure when full (`.send().await`
/// waits for the next drain); fire-and-forget sends (`write_pty`, `resize_pty`,
/// `refresh_changed_files`, `emit_status`) use `try_send` and drop on a full
/// channel — acceptable overload shedding, since reaching this depth means the
/// producer is far outrunning a 20-drains-per-second consumer. Kept a const,
/// like the broadcast capacities above, rather than user config: it is an
/// internal safety ceiling, not a preference.
const REQ_CHANNEL_CAPACITY: usize = 1024;

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
    auth_reload: Option<AuthReloadContext>,
) -> (EngineHandle, ActorLoopEnds) {
    let (req_tx, req_rx) = mpsc::channel::<EngineRequest>(REQ_CHANNEL_CAPACITY);
    // Status uses THREE channels driven from one place (the StatusEmitter):
    //  - `status_tx` (broadcast) delivers every status LIVE, so a transient
    //    pending flash ("Pulling…", "Launching…") is never coalesced away.
    //  - `status_clear_tx` (broadcast) delivers each key that was cleared so
    //    clients can dismiss the matching toast without waiting for a replacement.
    //    `None` = the anonymous slot; `Some(key)` = a named keyed op.
    //  - `status_snapshot_tx` (watch) always holds ALL OPEN statuses so a client
    //    connecting mid-status reads the full set once on connect (see
    //    `status_snapshot`), rather than waiting blank for the next update.
    let (status_tx, _status_rx) = broadcast::channel::<WireStatus>(256);
    let (status_clear_tx, _status_clear_rx) = broadcast::channel::<Option<String>>(256);
    let (status_snapshot_tx, status_snapshot_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
    // Config-reload notifier: the loop fires `()` on each successful reload and the
    // web layer's forwarder turns it into a `config.changed` event. A small buffer
    // is plenty — reloads are rare and the forwarder drains promptly.
    let (config_reload_tx, _config_reload_rx) = broadcast::channel::<()>(8);
    // Spine-change notifier: the loop fingerprints the spine each tick and fires a
    // `SpineChange` per changed side; the web forwarder turns it into a coarse
    // `projects.changed` / `sessions.changed` event. A small buffer is plenty — the
    // forwarder drains promptly and a `Lagged` recovery just re-emits both coarse
    // signals (idempotent refetches).
    let (spine_change_tx, _spine_change_rx) = broadcast::channel::<SpineChange>(64);
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    (
        EngineHandle {
            req_tx,
            status_tx: status_tx.clone(),
            status_clear_tx: status_clear_tx.clone(),
            status_snapshot_rx,
            config_reload_tx: config_reload_tx.clone(),
            spine_change_tx: spine_change_tx.clone(),
            shutdown_flag: Arc::clone(&shutdown_flag),
            has_active_processes: Arc::clone(&engine.has_active_processes),
        },
        ActorLoopEnds {
            req_rx,
            status_tx,
            status_clear_tx,
            status_snapshot_tx,
            config_reload_tx,
            spine_change_tx,
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

/// How long to wait for an agent provider to come up before failing a subscribe,
/// and the threshold after which a stale `Busy` entry is upgraded to `Warning`.
/// Shared with the TUI via `dux_core::statusline::BUSY_TIMEOUT`.
const LAUNCH_TIMEOUT: Duration = dux_core::statusline::BUSY_TIMEOUT;

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
    req_tx: mpsc::Sender<EngineRequest>,
    status_tx: broadcast::Sender<WireStatus>,
    status_clear_tx: broadcast::Sender<Option<String>>,
    status_snapshot_rx: watch::Receiver<Vec<KeyedWireStatus>>,
    /// Notifies on each successful config reload (see [`ActorLoopEnds`]). The web
    /// layer subscribes via [`EngineHandle::subscribe_config_reloads`] and re-emits
    /// a `config.changed` event so clients refetch `/api/v1/bootstrap`.
    config_reload_tx: broadcast::Sender<()>,
    /// Notifies on each projects/sessions spine change (see [`ActorLoopEnds`]). The
    /// web layer subscribes via [`EngineHandle::subscribe_spine_changes`] and
    /// re-emits a coarse `projects.changed` / `sessions.changed` event so clients
    /// refetch `/api/v1/spine`.
    spine_change_tx: broadcast::Sender<SpineChange>,
    /// Tripped when the server is tearing down (ReturnToTui, QuitProcess, or a
    /// `Shutdown` request). PTY forwarders poll it so their blocking
    /// `recv_timeout` loop exits promptly even when the engine — and therefore
    /// the std-mpsc `Sender` in the `PtyClient` reader thread — stays alive
    /// across the flip. Without this, a forwarder parked on a never-disconnecting
    /// channel would wedge the tokio blocking pool and hang the runtime teardown.
    shutdown_flag: Arc<AtomicBool>,
    /// Shared clone of the engine's `has_active_processes` flag, so the
    /// changed-files poller can read whether any agent PTY is live with a local
    /// atomic load (deciding its 2s-vs-10s cadence) instead of an actor
    /// round-trip. The engine writes it; the handle only reads it.
    has_active_processes: Arc<AtomicBool>,
}

// Axum state must be `Send + Sync`; prove the handle satisfies that here so a future
// regression (e.g. swapping a channel type) fails at compile time, not at the axum
// router boundary.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EngineHandle>();
};

impl EngineHandle {
    /// The teardown flag PTY forwarders poll. Cloned into each forwarder so a
    /// blocking `recv_timeout` loop can break within one timeout window once the
    /// server starts winding down, even though the underlying `PtyClient`'s
    /// `Sender` outlives the flip (ReturnToTui keeps PTYs alive). The same flag
    /// is held loop-side ([`ActorLoopEnds`]) and by `serve_with_engine`, which
    /// trips it the instant the engine loop returns.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown_flag)
    }

    pub fn subscribe_status(&self) -> broadcast::Receiver<WireStatus> {
        self.status_tx.subscribe()
    }

    /// Subscribe to the clear broadcast. Each item is the key that was removed:
    /// `None` = the anonymous slot cleared, `Some(key)` = a named keyed op. The
    /// `/ws/events` socket converts each into a `status_cleared` event.
    pub fn subscribe_status_clears(&self) -> broadcast::Receiver<Option<String>> {
        self.status_clear_tx.subscribe()
    }

    /// All currently open statuses (anonymous + keyed), from the snapshot watch.
    /// A client connecting mid-operation reads this once and sends each entry as
    /// a `Status` frame so it sees all active toasts immediately — e.g. a
    /// "Launching agent…" Busy that hasn't resolved yet — instead of a blank
    /// line until the next live update. An empty `Vec` means nothing is showing.
    pub fn status_snapshot(&self) -> Vec<KeyedWireStatus> {
        self.status_snapshot_rx.borrow().clone()
    }

    /// Like [`emit_status`] but attaches a correlation key so a later success,
    /// error, or clear on the same key replaces or dismisses the same toast.
    /// Prefer this over `emit_status` for any operation that has a keyed lifecycle
    /// (e.g. ACME certificate renewal, where a "Renewing…" busy should be
    /// replaced by a "Renewed." info on success and dismissed by `StatusCleared`).
    pub fn emit_keyed_status(&self, key: impl Into<String>, status: WireStatus) {
        self.emit_status(status.with_key(key));
    }

    /// Publish a status from a non-engine producer (the ACME certificate-lifecycle
    /// task) THROUGH the shared status controller — not directly onto the broadcast
    /// — so it auto-clears on the same tone-aware policy as every other status and
    /// can never linger. The engine loop drains this and emits it via its
    /// `StatusEmitter`. A no-op if the engine loop has already exited.
    pub fn emit_status(&self, status: WireStatus) {
        // `try_send` (not `send().await`): this is sync fire-and-forget, called
        // from non-engine producers (the ACME certificate-lifecycle task, the
        // changed-files `ChangesService`). On a full channel the status is dropped
        // — only under extreme overload — but a dropped status is worth a
        // breadcrumb, so log the Full case with the status's tone/key so the
        // operator can tell WHICH producer's update went missing. A Closed channel
        // means the engine is already gone (normal shutdown), so it stays silent.
        let tone = status.tone.clone();
        let key = status.key.clone();
        if let Err(mpsc::error::TrySendError::Full(_)) =
            self.req_tx.try_send(EngineRequest::EmitStatus(status))
        {
            dux_core::logger::warn(&format!(
                "engine request channel full: dropped a non-engine status update \
                 (tone={tone}, key={key:?})"
            ));
        }
    }

    pub async fn apply_wire(&self, command: WireCommand) -> Result<WireCommandOutcome, String> {
        self.apply_wire_scoped(command, StatusScope::All).await
    }

    /// Like [`apply_wire`](Self::apply_wire) but tags the command with the
    /// originating connection's [`StatusScope`], so any statuses it mints (the
    /// synchronous outcome, deferred busies/finals, worker busies) are delivered
    /// only to that connection. `apply_wire` delegates here with
    /// [`StatusScope::All`] (broadcast), so existing callers are unchanged.
    pub async fn apply_wire_scoped(
        &self,
        command: WireCommand,
        origin: StatusScope,
    ) -> Result<WireCommandOutcome, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::ApplyWire(command, tx, origin))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// Bump and return the next monotonic changed-files revision for `session_id`
    /// (one actor round-trip). The counter is persisted in SQLite, so it never
    /// resets across restarts. Returns `0` only if the engine thread is gone.
    pub async fn next_changes_rev(&self, session_id: String) -> u64 {
        let (tx, rx) = oneshot::channel();
        let sid = session_id.clone();
        if self
            .req_tx
            .send(EngineRequest::NextChangesRev(session_id, tx))
            .await
            .is_err()
        {
            // The engine thread is gone (shutdown). Returning the 0 fallback is
            // safe (the client's `rev >=` apply guard treats it as redundant), but
            // log it so a spurious non-advancing rev is explainable.
            dux_core::logger::warn(&format!(
                "next_changes_rev for session {sid}: engine thread gone; using rev 0 fallback"
            ));
            return 0;
        }
        match rx.await {
            Ok(rev) => rev,
            Err(_) => {
                dux_core::logger::warn(&format!(
                    "next_changes_rev for session {sid}: engine reply dropped; using rev 0 fallback"
                ));
                0
            }
        }
    }

    /// Whether any agent PTY is currently live, read as a local atomic load (no
    /// actor round-trip). The changed-files poller uses this to pick its cadence
    /// (2s when an agent is active, else 10s).
    pub fn has_active_processes(&self) -> bool {
        self.has_active_processes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn subscribe_pty(&self, session_id: String) -> Result<PtySubscription, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::SubscribePty(session_id, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub fn write_pty(&self, session_id: String, bytes: Vec<u8>) {
        // `try_send`: keystrokes are fire-and-forget from a sync caller. A full
        // channel means the producer is flooding far past the engine's drain
        // rate; shedding the write then is the intended overload behaviour (the
        // bounded channel + WS frame-size limit together cap memory).
        let _ = self
            .req_tx
            .try_send(EngineRequest::WritePty(session_id, bytes));
    }

    pub fn resize_pty(&self, session_id: String, rows: u16, cols: u16) {
        // `try_send`: a resize dropped under overload is self-correcting (the next
        // resize re-establishes the size); no need to backpressure a sync caller.
        let _ = self
            .req_tx
            .try_send(EngineRequest::ResizePty(session_id, rows, cols));
    }

    pub async fn subscribe_terminal(&self, terminal_id: String) -> Result<PtySubscription, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::SubscribeTerminal(terminal_id, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    pub async fn create_terminal(&self, session_id: String) -> Result<(String, String), String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(EngineRequest::CreateTerminal(session_id, tx))
            .await
            .map_err(|_| "engine thread gone".to_string())?;
        rx.await.map_err(|_| "engine reply dropped".to_string())?
    }

    /// The session id that owns companion terminal `terminal_id`, or `None` when the
    /// terminal is unknown or the engine thread is gone. The nested terminal PTY
    /// socket and the `DELETE .../terminals/:tid` route use this to enforce that the
    /// terminal belongs to the path's session before acting on it.
    pub async fn terminal_session(&self, terminal_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::TerminalSession(terminal_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Gracefully wind down the engine: SIGTERM the agent/terminal children so
    /// CLIs can save state for a later resume, then stop the engine thread.
    /// Errors are ignored — if the thread is already gone, shutdown has already
    /// happened.
    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self.req_tx.send(EngineRequest::Shutdown(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }

    pub async fn session_worktree(&self, session_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SessionWorktree(session_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Snapshot the build-/config-static bootstrap projection for
    /// `GET /api/v1/bootstrap`. `None` if the engine is gone (the handler then
    /// returns 503), distinguishing a dead engine from a real empty payload.
    pub async fn bootstrap(&self) -> Option<dux_core::viewmodel::BootstrapView> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::Bootstrap(tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Snapshot the projects/sessions/sidebar spine for `GET /api/v1/spine` and the
    /// thin per-resource reads. `None` if the engine is gone (the handler then
    /// returns 503), distinguishing a dead engine from a real empty payload.
    pub async fn spine(&self) -> Option<dux_core::viewmodel::SpineView> {
        let (tx, rx) = oneshot::channel();
        if self.req_tx.send(EngineRequest::Spine(tx)).await.is_err() {
            return None;
        }
        rx.await.ok()
    }

    /// The pre-serialized whole-spine JSON for `GET /api/v1/spine`, served from the
    /// loop's cache. `None` if the engine is gone (the handler then returns 503),
    /// distinguishing a dead engine from a real payload.
    pub async fn spine_json(&self) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SpineJson(tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Project ONLY the session with `id` for `GET /api/v1/sessions/:id`. The outer
    /// `Option` is `None` if the engine is gone (503); the inner `None` means the
    /// session id is unknown (404).
    pub async fn session(&self, id: String) -> Option<Option<dux_core::viewmodel::SessionView>> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::Session(id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok()
    }

    /// Resolve the session id produced by create op `op_id` (returned in
    /// `WireCommandOutcome.created_op_id`). `None` while the create is still in
    /// flight, the entry has expired, or the engine thread is gone.
    pub async fn created_session_for_op(&self, op_id: String) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::CreatedSessionForOp(op_id, tx))
            .await
            .is_err()
        {
            return None;
        }
        rx.await.ok().flatten()
    }

    /// Subscribe to config-reload notifications. The web layer forwards each into a
    /// `config.changed` event on its event bus so subscribed clients refetch
    /// `/api/v1/bootstrap`.
    pub fn subscribe_config_reloads(&self) -> broadcast::Receiver<()> {
        self.config_reload_tx.subscribe()
    }

    /// Subscribe to projects/sessions spine-change notifications. The web layer
    /// forwards each into a coarse `projects.changed` / `sessions.changed` event on
    /// its event bus so subscribed clients refetch `/api/v1/spine`.
    pub fn subscribe_spine_changes(&self) -> broadcast::Receiver<SpineChange> {
        self.spine_change_tx.subscribe()
    }

    /// The configured preferred editor name for the "open in editor" action
    /// (`config.editor.default`). Empty if the engine is gone — the handler then
    /// falls back to the first detected editor.
    pub async fn editor_default(&self) -> String {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::EditorDefault(tx))
            .await
            .is_err()
        {
            return String::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Fire-and-forget: ask the engine to recompute changed-files for `worktree`
    /// (after an HTTP git mutation). The refreshed lists are served by the REST
    /// changed-files read; nothing to await here.
    pub fn refresh_changed_files(&self, worktree: String) {
        // `try_send`: a dropped refresh under overload self-heals — the periodic
        // changed-files poller recomputes the lists on its next pass regardless.
        let _ = self
            .req_tx
            .try_send(EngineRequest::RefreshChangedFiles(worktree));
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
            .await
            .is_err()
        {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    /// Resolve a session's startup-command-log context: the dux paths and the
    /// session's owning project id. Instant lookup — the log directory listing /
    /// file read runs off-thread in the caller (the `project_worktree_inputs`
    /// precedent). `None` when the session id is unknown.
    pub async fn session_startup_log_context(
        &self,
        session_id: String,
    ) -> Option<(dux_core::config::DuxPaths, String)> {
        let (tx, rx) = oneshot::channel();
        if self
            .req_tx
            .send(EngineRequest::SessionStartupLogContext(session_id, tx))
            .await
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
    auth_reload: AuthReloadContext,
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
        status_tx: thread_status_tx,
        status_clear_tx,
        status_snapshot_tx,
        config_reload_tx,
        spine_change_tx,
        shutdown_flag,
        auth_reload,
    } = ends;
    // Route every status through the shared KeyedStatusController so the web gets
    // the SAME auto-clear + pending→final behaviour as the TUI from one place.
    // The emitter upserts by key, broadcasts each status LIVE, and keeps the Vec
    // snapshot watch in sync so connecting clients see all open toasts at once.
    // Keeping the binding name `thread_status_tx` and a `send` method means the
    // loop's existing call sites need no changes. `tick` (called once per
    // iteration below) expires timed-out Info entries, upgrades stale Busy→Warning,
    // and pushes cleared keys onto `clear_tx`. The timeout is captured at startup
    // (like the TUI), so changing `status_clear_seconds` takes effect on the next
    // server start.
    let mut thread_status_tx = StatusEmitter::new(
        thread_status_tx,
        status_clear_tx,
        status_snapshot_tx,
        Duration::from_secs(engine.config.ui.status_clear_seconds as u64),
    );
    // Subscribes waiting for their provider to come up via the worker-event drain.
    let mut pending: Vec<PendingSubscribe> = Vec::new();
    // Last-seen fingerprints of the two spine halves, so a coarse
    // `projects.changed` / `sessions.changed` event fires only when the projection
    // actually changes (not every idle tick). Initialized from the current state so
    // the first tick does not emit a spurious change for an unchanged spine.
    let (mut prev_projects_fp, mut prev_sessions_fp) = spine_fingerprints(&engine);
    // Cache of the whole-spine JSON served to `GET /api/v1/spine`. Rebuilt only
    // when a fingerprint check detects a real change (below), so per-client reads
    // clone the cached string instead of re-projecting + re-serializing the spine.
    // Seeded now so a request before the first change still serves a valid body.
    let mut spine_json_cache =
        serde_json::to_string(&engine.spine()).unwrap_or_else(|_| "{}".to_string());
    // Tick counter for throttling the spine fingerprint/cache check (see
    // SPINE_CHECK_TICK_INTERVAL) so it runs ~every 250ms rather than every tick.
    let mut tick_count: u64 = 0;
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
            // dispatch. A lookup FAILURE instead produced a keyed error Status
            // (resolving the PR-lookup op), already surfaced by the wire_statuses
            // drain above. On SUCCESS the followup hands off to the create busy and
            // returns the PR-lookup op's clear key so its spinner is dismissed.
            let pr_followup = engine.drive_pr_lookup_followup(&reaction);
            for status in pr_followup.statuses {
                let _ = thread_status_tx.send(status);
            }
            for key in pr_followup.clear_keys {
                thread_status_tx.clear(key);
            }

            // A reconnect / force-restart launch reported back: resolve the web
            // launch op (Engine::pending_web_launch_ops) so its "Launching…" /
            // "Starting fresh…" busy is replaced by the same-key final (or cleared
            // when the session vanished). Create-kind launch finals are resolved
            // engine-side and ride the wire_statuses drain above.
            let launch_followup = engine.drive_web_launch_followup(&reaction);
            for status in launch_followup.statuses {
                let _ = thread_status_tx.send(status);
            }
            for key in launch_followup.clear_keys {
                thread_status_tx.clear(key);
            }
            // A StatusOp resolved to `Final::Clear`: dismiss the keyed toast.
            if let dux_core::engine::EventReaction::ClearStatus(key) = &reaction {
                thread_status_tx.clear(key.clone());
            }

            // A project mutation just updated SQLite + in-memory projects; mirror
            // it into the portable config.toml so a later TUI start doesn't clobber it.
            // Skip for `Added` — that arm already wrote config inline in command.rs,
            // and Skip for `PersistenceFailed` — nothing was saved.
            if let EventReaction::ProjectPersistenceOutcome(outcome) = &reaction
                && !matches!(
                    outcome.view,
                    ProjectPersistenceView::PersistenceFailed { .. }
                        | ProjectPersistenceView::Added { .. }
                )
                && let Err(e) = engine.persist_projects_to_config()
            {
                let _ = thread_status_tx.send(WireStatus::keyed(
                    "config-write",
                    "error",
                    format!("Saved to the database, but config.toml could not be updated: {e:#}"),
                ));
            }

            // A reload worker re-read config.toml; apply the new config to the
            // running engine. This consumes `reaction`, so it MUST be the last
            // use of it in the loop body (all `&reaction` borrows above end
            // first). `ApplyReloadedConfig` and `ProjectPersistenceOutcome` are
            // distinct variants, so consuming here never skips the project sync.
            //
            // The reload follow-up reaction may arrive WRAPPED in a `Multi` when
            // config-mutating commands were deferred during the reload (the
            // engine folds the `ApplyReloadedConfig` in with the deferred saves'
            // status reactions). Pull the `ApplyReloadedConfig` out of either the
            // bare or the wrapped form so the auth-gate rebuild + server-restart
            // warning always run — an auth/user change made during a reload must
            // take effect without a restart. The deferred saves' own status
            // reactions were already surfaced by the `wire_statuses_from_reaction`
            // drain above (it flattens `Multi`).
            if let Some(config) = take_apply_reloaded_config(reaction) {
                // Capture the rebind-relevant [server]/[server.acme] settings
                // BEFORE the swap so we can tell whether the reload touched
                // anything that only takes effect at startup (listeners are
                // bound once; reload-config never rebinds). Comparing here — the
                // arm already holds both the running config (pre-swap) and the
                // incoming one — keeps the detection next to the auth reload hook.
                let server_settings_changed =
                    server_rebind_settings_changed(&engine.config.server, &config.server);
                match engine.apply_reloaded_config(*config) {
                    Ok(()) => {
                        // Signal the web layer that config-static state changed so
                        // it emits a `config.changed` event and clients refetch
                        // `/api/v1/bootstrap`. Fire-and-forget: an `Err` only means
                        // no forwarder is listening (e.g. the TUI flip), which is
                        // fine.
                        let _ = config_reload_tx.send(());
                        // Rebuild the login gate's shared snapshot from the
                        // freshly-applied `[auth]` users so credential changes
                        // (add/remove/change password via config or the TUI
                        // palette) take effect without a server restart. The
                        // `--disable-auth` flag captured at startup is preserved.
                        let mut auth_refused = false;
                        if let Some(ctx) = auth_reload.as_ref()
                            && let Ok(mut guard) = ctx.shared.write()
                        {
                            // Rebuild from the new config. On a reachable bind
                            // (public OR Tailscale) the rebuild REFUSES a gate
                            // downgrade (last user removed) and keeps the prior
                            // users; on a host-only loopback bind it downgrades
                            // with a warning. See `AuthState::rebuild`.
                            let prev = guard.clone();
                            let (next, refused) = crate::auth::AuthState::rebuild(
                                &prev,
                                &engine.config.auth.users,
                                ctx.disable_auth,
                                ctx.host_only,
                            );
                            *guard = next;
                            auth_refused = refused;
                        }
                        // When the rebuild REFUSED the downgrade the `[auth]`
                        // change was deliberately NOT applied, so a plain
                        // "settings are active" status would mislead. Surface the
                        // refusal (and how to actually run open) in a warn-tone
                        // status instead.
                        let status = if auth_refused {
                            WireStatus::new(
                                "warning",
                                "Configuration reloaded, but removing the last login user was \
                                 refused: this server is reachable from other devices (a public \
                                 or Tailscale bind) and will not drop its login gate while \
                                 running. Restart the server to apply.",
                            )
                        } else {
                            WireStatus::new(
                                "info",
                                "Configuration reloaded. New settings are active.",
                            )
                        };
                        // Echo the reload outcome on the CLI console too (a refusal
                        // reads as a warning, a clean reload as info); the WS
                        // broadcast carries the same text. A no-op console (flip/
                        // tests) emits nothing.
                        if let Some(ctx) = auth_reload.as_ref() {
                            ctx.console.reload(&status.message, auth_refused);
                        }
                        let _ = thread_status_tx.send(status);

                        // The new config WAS applied to the engine, but the
                        // listen/TLS sections only bind at startup — a reload
                        // cannot rebind them. Warn so the user knows a restart is
                        // needed for those specific changes to take effect. This
                        // is a separate concern from the auth-refusal status
                        // above, so it rides as its own warn-tone status.
                        if server_settings_changed {
                            let drift = "Server listen/TLS settings changed in config — restart \
                                 the server to apply them.";
                            if let Some(ctx) = auth_reload.as_ref() {
                                ctx.console.reload(drift, true);
                            }
                            let _ = thread_status_tx.send(WireStatus::new("warning", drift));
                        }
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
        // toward the `working` projection in the spine read below. This is the
        // single poll site for the web surface (the TUI run loop is the single
        // poll site for the other surface; the two never run at once).
        engine.poll_pty_activity();

        // Refresh companion-terminal foreground commands so the spine's
        // `foreground_cmd` tracks what's running. The engine throttles this by
        // wall-clock (~2s), so calling it every tick is cheap.
        engine.refresh_terminal_foregrounds();

        // Reap agent/terminal PTYs whose child process exited so they stop
        // lingering in `providers`/`companion_terminals` and disappear from the
        // spine, broadcasting a status for each so web clients learn.
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

        // The projects/sessions/sidebar spine is signaled via coarse events.
        // Fingerprint each half and emit only the
        // changed side(s). A failed send means no web forwarder is listening (e.g.
        // the TUI flip), which is fine. Throttled to every Nth tick (see
        // SPINE_CHECK_TICK_INTERVAL) so idle serialization cost drops ~5x; every
        // real change still fires its coarse event within the throttle window.
        tick_count = tick_count.wrapping_add(1);
        if tick_count.is_multiple_of(SPINE_CHECK_TICK_INTERVAL) {
            let (projects_fp, sessions_fp) = spine_fingerprints(&engine);
            let mut spine_changed = false;
            if projects_fp != prev_projects_fp {
                prev_projects_fp = projects_fp;
                let _ = spine_change_tx.send(SpineChange::Projects);
                spine_changed = true;
            }
            if sessions_fp != prev_sessions_fp {
                prev_sessions_fp = sessions_fp;
                let _ = spine_change_tx.send(SpineChange::Sessions);
                spine_changed = true;
            }
            // Refresh the cached `GET /api/v1/spine` JSON only when a half actually
            // changed, so the common case (no change) skips the full serialization.
            if spine_changed {
                spine_json_cache =
                    serde_json::to_string(&engine.spine()).unwrap_or_else(|_| "{}".to_string());
            }
        }

        let mut disconnected = false;
        loop {
            match req_rx.try_recv() {
                Ok(EngineRequest::SubscribePty(session_id, reply)) => {
                    handle_subscribe(&mut engine, &mut pending, session_id, reply);
                }
                Ok(EngineRequest::SpineJson(reply)) => {
                    // Serve the loop-local cache (handled here, not in
                    // `handle_request`, which has no access to it).
                    let _ = reply.send(spine_json_cache.clone());
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
                Ok(req) => {
                    handle_request(&mut engine, req, &mut thread_status_tx, &config_reload_tx)
                }
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
        // Expire a timed-out transient status and broadcast the cleared state to
        // every connected client. Busy/warning/error are left untouched.
        thread_status_tx.tick(Instant::now());
        thread::sleep(TICK);
    }
    engine
}

/// Wraps the WS status channels so every status flows through one shared
/// [`KeyedStatusController`]: [`send`](Self::send) upserts the status by key,
/// refreshes the Vec snapshot watch (all open statuses, for clients connecting
/// mid-operation), and broadcasts it LIVE so a transient pending flash is never
/// coalesced away; [`tick`](Self::tick) — called once per loop iteration —
/// expires timed-out entries, pushes removed keys onto `clear_tx` (so Task 7's
/// WS forwarder sends `StatusCleared` frames), and upgrades stale Busy entries
/// to Warning (busy-timeout). The `send` method name and broadcast return type
/// let the loop's existing call sites stay unchanged.
struct StatusEmitter {
    tx: broadcast::Sender<WireStatus>,
    clear_tx: broadcast::Sender<Option<String>>,
    snapshot_tx: watch::Sender<Vec<KeyedWireStatus>>,
    controller: KeyedStatusController,
    /// Most recent generation for each keyed status so `clear` can guard
    /// against dismissing a newer status placed on the same key by a
    /// concurrent operation (e.g. a rapid retry during commit-msg generation).
    generations: std::collections::HashMap<String, Generation>,
}

impl StatusEmitter {
    fn new(
        tx: broadcast::Sender<WireStatus>,
        clear_tx: broadcast::Sender<Option<String>>,
        snapshot_tx: watch::Sender<Vec<KeyedWireStatus>>,
        clear_after: Duration,
    ) -> Self {
        Self {
            tx,
            clear_tx,
            snapshot_tx,
            controller: KeyedStatusController::with_clear_after(clear_after),
            generations: std::collections::HashMap::new(),
        }
    }

    /// Upsert the status in the controller (keyed or anonymous), refresh the
    /// Vec snapshot, then broadcast it live. Returns the broadcast `send` result
    /// so the call sites keep discarding it with `let _ =` exactly as before.
    fn send(
        &mut self,
        status: WireStatus,
    ) -> Result<usize, broadcast::error::SendError<WireStatus>> {
        let tone = StatusTone::from_wire(&status.tone);
        let generation = self.controller.set_scoped(
            Instant::now(),
            status.key.clone(),
            tone,
            status.message.as_str(),
            status.scope.clone(),
        );
        if let Some(ref k) = status.key {
            self.generations.insert(k.clone(), generation);
        }
        let _ = self.snapshot_tx.send(self.controller.snapshot());
        self.tx.send(status)
    }

    /// Explicitly clear a keyed entry (remove from the controller, push the
    /// key onto `clear_tx` so the WS forwarder sends a `StatusCleared` frame,
    /// refresh the snapshot). Guards with the generation stored when the busy
    /// was emitted so a concurrent in-flight cannot be prematurely dismissed.
    fn clear(&mut self, key: String) {
        let generation = self.generations.get(&key).copied();
        if self.controller.clear(&key, generation) {
            self.generations.remove(&key);
            let _ = self.snapshot_tx.send(self.controller.snapshot());
            let _ = self.clear_tx.send(Some(key));
        }
    }

    /// Expire timed-out entries (auto-clear Info, upgrade stale Busy→Warning).
    /// Pushes cleared keys onto `clear_tx` (for the WS forwarder's
    /// `StatusCleared` frames) and broadcasts upgraded entries as live `WireStatus`
    /// updates. Short-circuits when nothing changed so idle ticks cost nothing.
    fn tick(&mut self, now: Instant) {
        let changes = self.controller.tick(now, LAUNCH_TIMEOUT);
        if changes.cleared_keys.is_empty() && changes.upgraded.is_empty() {
            return;
        }
        let _ = self.snapshot_tx.send(self.controller.snapshot());
        for key in changes.cleared_keys {
            let _ = self.clear_tx.send(key);
        }
        for up in changes.upgraded {
            let _ = self.tx.send(WireStatus {
                key: up.key,
                tone: up.tone,
                message: up.message,
                scope: up.scope,
            });
        }
    }
}

/// Fingerprint the two halves of the projected spine as `(projects, sessions)`
/// JSON strings, for the loop's coarse change detection.
///
/// The sidebar is deliberately EXCLUDED from both fingerprints: it is fully
/// DERIVED from projects + sessions, so every sidebar input change is already
/// captured by one of the two halves — a project name/`path_missing` change moves
/// the projects fingerprint, and a session `project_id`/order/orphan transition
/// moves the sessions fingerprint. Folding the sidebar (which embeds project
/// fields) into the sessions half instead made a PROJECT-only change spuriously
/// fire `sessions.changed`. Since the client refetches the whole `/spine` on
/// either event, the sidebar still re-fetches correctly on whichever side fired.
fn spine_fingerprints(engine: &Engine) -> (String, String) {
    let spine = engine.spine();
    let projects = serde_json::to_string(&spine.projects).unwrap_or_else(|_| "[]".to_string());
    let sessions = serde_json::to_string(&spine.sessions).unwrap_or_else(|_| "[]".to_string());
    (projects, sessions)
}

fn handle_request(
    engine: &mut Engine,
    req: EngineRequest,
    status_tx: &mut StatusEmitter,
    config_reload_tx: &broadcast::Sender<()>,
) {
    match req {
        EngineRequest::ApplyWire(cmd, reply, origin) => {
            // A config-static mutation (macros / global env / Changes-pane flag)
            // eager-saves and adopts the change in place — there is no disk reload
            // to drive the usual `config.changed` signal, so we fire it ourselves
            // below once the command succeeds. Capture this BEFORE `cmd` is moved
            // into `apply_wire`.
            let mutates_config = cmd.mutates_config_static();
            // Tag every status this command mints with the originating
            // connection. The engine reads `current_origin` at each mint site
            // (the synchronous outcome, deferred busies/finals, worker busies);
            // reset to `All` after so a later spontaneous status still broadcasts.
            engine.current_origin = origin;
            let res = engine.apply_wire(cmd).map_err(|e| e.to_string());
            engine.current_origin = StatusScope::All;
            // On a successful config-static mutation, signal the web layer's
            // forwarder so it emits `config.changed` and every `config`-subscribed
            // client refetches `/api/v1/bootstrap`. Fire-and-forget: an `Err` only
            // means no forwarder is listening (e.g. the TUI flip), which is fine.
            if res.is_ok() && mutates_config {
                let _ = config_reload_tx.send(());
            }
            // ALSO route the synchronous command-result status through the shared
            // controller — broadcast to EVERY client and auto-cleared on the same
            // policy as engine statuses — instead of leaving it only on the
            // requesting client's status line, where it never cleared (and could
            // be wiped early by an unrelated status's expiry). The reply still
            // carries the status for the requester's instant ack (and the Err path
            // it needs to revert an optimistic reorder); the requester then sets
            // the same value from both the reply and the broadcast, which is a
            // harmless idempotent set.
            if let Ok(outcome) = &res
                && let Some(status) = &outcome.status
            {
                let _ = status_tx.send(status.clone());
            }
            let _ = reply.send(res);
        }
        EngineRequest::EmitStatus(status) => {
            let _ = status_tx.send(status);
        }
        // SubscribePty is handled inline in the loop (it needs `&mut pending`).
        EngineRequest::SubscribePty(_, _) => unreachable!("SubscribePty handled in the loop"),
        // Shutdown is handled inline in the loop (it must stop the thread).
        EngineRequest::Shutdown(_) => unreachable!("Shutdown handled in the loop"),
        EngineRequest::WritePty(id, bytes) => {
            let wrote =
                pty_for(engine, &id).is_some_and(|client| client.write_bytes(&bytes).is_ok());
            // Record keystrokes that actually reached an agent PTY (not a
            // companion terminal, not an empty frame) so the user's own echoed
            // typing doesn't read as the agent "working".
            if wrote && !bytes.is_empty() && engine.providers.contains_key(&id) {
                engine.note_pty_input(&id);
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
        EngineRequest::TerminalSession(terminal_id, reply) => {
            let owner = engine
                .companion_terminals
                .get(&terminal_id)
                .map(|t| t.session_id.clone());
            let _ = reply.send(owner);
        }
        EngineRequest::SessionWorktree(session_id, reply) => {
            let worktree = engine
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .map(|s| s.worktree_path.clone());
            let _ = reply.send(worktree);
        }
        EngineRequest::Bootstrap(reply) => {
            let _ = reply.send(engine.bootstrap());
        }
        EngineRequest::Spine(reply) => {
            let _ = reply.send(engine.spine());
        }
        // SpineJson is handled inline in the loop (it serves the loop-local cache).
        EngineRequest::SpineJson(_) => unreachable!("SpineJson handled in the loop"),
        EngineRequest::Session(id, reply) => {
            let _ = reply.send(engine.session_view(&id));
        }
        EngineRequest::CreatedSessionForOp(op_id, reply) => {
            let _ = reply.send(engine.created_session_for_op(&op_id));
        }
        EngineRequest::NextChangesRev(session_id, reply) => {
            // Single chokepoint over the engine's SQLite connection. On a DB
            // error fall back to 0 (a non-advancing rev), which the client's
            // `rev >=` apply guard treats as a redundant refetch rather than a
            // crash; the error is logged for diagnosis.
            let rev = engine
                .session_store
                .next_changes_rev(&session_id)
                .unwrap_or_else(|e| {
                    dux_core::logger::error(&format!(
                        "next_changes_rev failed for session {session_id}: {e}"
                    ));
                    0
                });
            let _ = reply.send(rev);
        }
        EngineRequest::EditorDefault(reply) => {
            let _ = reply.send(engine.config.editor.default.clone());
        }
        EngineRequest::RefreshChangedFiles(worktree) => {
            // Spawn the off-thread refresh unconditionally. If this worktree is
            // not the currently-watched one, the resulting `ChangedFilesReady`
            // (worktree-tagged) is dropped by `events.rs`. In practice the git
            // HTTP handler only refreshes the worktree it just mutated, which is
            // normally the watched one.
            engine.spawn_changed_files_refresh(std::path::PathBuf::from(worktree));
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
        EngineRequest::SessionStartupLogContext(session_id, reply) => {
            let context = engine
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .map(|session| (engine.paths.clone(), session.project_id.clone()));
            let _ = reply.send(context);
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
    // Use the SAME completion message the TUI shows on reconnect-ready (via
    // Engine::agent_reconnect_status_message) rather than a static "attaching…"
    // placeholder. The launch-ready reaction echoes this status_message back as
    // the final status; echoing a placeholder is what left the status line stuck
    // on "Attaching to agent" forever after the agent had already attached.
    let status_message = engine.agent_reconnect_status_message(&session, resume);
    let request = engine.build_agent_launch_request(
        session,
        resume,
        (24, 80),
        AgentLaunchKind::Reconnect { status_message },
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
    async fn apply_wire_toggle_reflects_in_spine_and_emits_sessions_change() {
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

        // Subscribe to spine changes BEFORE the mutation so we observe the loop's
        // fingerprint detection fire `SpineChange::Sessions`.
        let mut spine_rx = handle.subscribe_spine_changes();

        let outcome = handle
            .apply_wire(WireCommand::ToggleAgentAutoReopen {
                session_id: "s1".to_string(),
                enabled: true,
            })
            .await
            .expect("apply");
        assert!(outcome.status.is_some());

        // The session toggle changes the sessions half of the spine, so the loop
        // emits `SpineChange::Sessions`.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let saw_sessions = loop {
            match tokio::time::timeout(std::time::Duration::from_millis(200), spine_rx.recv()).await
            {
                Ok(Ok(SpineChange::Sessions)) => break true,
                Ok(Ok(SpineChange::Projects)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break false,
                Ok(Err(_)) | Err(_) => {}
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
        };
        assert!(saw_sessions, "toggle must fire SpineChange::Sessions");

        // The spine read now reflects the toggle.
        let spine = handle.spine().await.expect("spine");
        let session = spine
            .sessions
            .iter()
            .find(|s| s.id == "s1")
            .expect("session s1 present");
        assert!(
            session.auto_reopen_enabled,
            "the spine must reflect the auto-reopen toggle"
        );
    }

    #[tokio::test]
    async fn apply_wire_status_reaches_the_shared_watch() {
        // A synchronous command result is ALSO published on the shared status
        // watch (not just returned to the requester), so it reaches every client
        // AND a client connecting right after sees it via the snapshot. The
        // council caught that command results previously bypassed the controller.
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

        // Subscribe to the LIVE broadcast before issuing the command so we can
        // prove the status reaches already-connected clients — not just the
        // snapshot. (A regression that dropped the broadcast send but kept the
        // snapshot update would otherwise pass.)
        let mut status_rx = handle.subscribe_status();

        let outcome = handle
            .apply_wire(WireCommand::ToggleAgentAutoReopen {
                session_id: "s1".to_string(),
                enabled: true,
            })
            .await
            .expect("apply");
        // The reply still carries the status (the requester's instant ack)…
        let want = outcome.status.expect("command produced a status").message;

        // …apply_wire updates the watch before it replies, so the shared snapshot
        // a newly-connected client would read now holds the same status. The
        // snapshot is now a Vec of all open statuses; the anonymous slot (key=None)
        // is the expected entry for unkeyed command-result statuses.
        let snap = handle.status_snapshot();
        assert!(
            snap.iter().any(|s| s.message == want),
            "snapshot did not contain the expected status: {snap:?}"
        );

        // …AND it is delivered live on the broadcast to every connected client.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let delivered = loop {
            match tokio::time::timeout(std::time::Duration::from_millis(200), status_rx.recv())
                .await
            {
                Ok(Ok(s)) if s.message == want => break true,
                Ok(Ok(_)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break false,
                Ok(Err(_)) | Err(_) => {}
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
        };
        assert!(
            delivered,
            "command status was not broadcast to live clients"
        );
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

    // -----------------------------------------------------------------------
    // StatusEmitter unit tests (no Engine, no I/O — channels only)
    // -----------------------------------------------------------------------

    /// Build a `StatusEmitter` directly from inline channels (no engine needed)
    /// so the shape of the struct and its snapshot behaviour can be tested
    /// without spawning a thread. Mirrors the channel setup in
    /// `build_actor_channels_with_auth`.
    fn make_emitter() -> (StatusEmitter, watch::Receiver<Vec<KeyedWireStatus>>) {
        let (tx, _rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, _crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        let emitter = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::with_clear_after(Duration::from_secs(6)),
            generations: std::collections::HashMap::new(),
        };
        (emitter, snap_rx)
    }

    #[test]
    fn emitter_snapshot_holds_all_open_keyed_statuses() {
        // Both a keyed "pull" busy and a keyed "launch" busy must appear in the
        // snapshot together so a reconnecting client receives every active toast,
        // not just the latest one.
        let (mut e, snap_rx) = make_emitter();
        let _ = e.send(WireStatus::keyed("pull", "busy", "Pulling\u{2026}"));
        let _ = e.send(WireStatus::keyed("launch", "busy", "Launching\u{2026}"));
        let snap = snap_rx.borrow().clone();
        assert_eq!(snap.len(), 2, "snapshot must list both open busys");
        assert!(
            snap.iter().any(|s| s.key.as_deref() == Some("pull")),
            "pull must appear in snapshot"
        );
        assert!(
            snap.iter().any(|s| s.key.as_deref() == Some("launch")),
            "launch must appear in snapshot"
        );
    }

    #[test]
    fn emitter_tick_clears_expired_info_and_broadcasts_clear_key() {
        // An expired Info entry is removed from the snapshot AND its key is
        // pushed onto the clear broadcast so the WS forwarder can send
        // `StatusCleared`.
        let (tx, _rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, mut crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        let mut e = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::with_clear_after(Duration::from_secs(6)),
            generations: std::collections::HashMap::new(),
        };
        // One keyed Info that will expire and one anonymous Info that will too.
        let _ = e.send(WireStatus::keyed("commit", "info", "Committed."));
        let _ = e.send(WireStatus::new("info", "Saved."));
        assert_eq!(snap_rx.borrow().len(), 2, "both statuses in snapshot");

        // Advance wall-clock past clear_after — use the controller's tick directly.
        // Simulate by calling tick with a far-future instant.
        let far_future = Instant::now() + Duration::from_secs(100);
        e.tick(far_future);

        // Snapshot must now be empty.
        assert!(
            snap_rx.borrow().is_empty(),
            "snapshot must be empty after expiry"
        );

        // Both cleared keys must have been broadcast.
        let mut cleared = Vec::new();
        while let Ok(key) = crx.try_recv() {
            cleared.push(key);
        }
        assert_eq!(cleared.len(), 2, "both keys must be broadcast on clear_tx");
        assert!(
            cleared.contains(&Some("commit".to_string())),
            "keyed clear expected"
        );
        assert!(cleared.contains(&None), "anonymous clear expected");
    }

    #[test]
    fn emitter_clear_is_a_no_op_when_a_newer_status_replaced_the_key() {
        // LOW 5: a commit-msg clear must not dismiss a concurrent same-key
        // generate's busy. The emitter stores the generation from the busy it
        // emitted and guards the clear with it; once a newer status (a fresh
        // generate) replaces the key, the stale clear becomes a no-op.
        let (mut e, snap_rx) = make_emitter();
        let key = "commit-msg:s1";

        // First generate sets the busy and remembers its generation.
        let _ = e.send(WireStatus::keyed(key, "busy", "Generating\u{2026}"));
        let stale_generation = *e.generations.get(key).expect("busy stored a generation");

        // A concurrent second generate replaces the key with a newer generation.
        let _ = e.send(WireStatus::keyed(key, "busy", "Generating again\u{2026}"));
        assert_ne!(
            *e.generations.get(key).unwrap(),
            stale_generation,
            "the replacement must bump the generation"
        );

        // Simulate the FIRST generate's clear arriving late by restoring the
        // stale generation it captured, then clearing.
        e.generations.insert(key.to_string(), stale_generation);
        e.clear(key.to_string());

        // The newer busy must still be present — the stale clear was a no-op.
        let snap = snap_rx.borrow().clone();
        assert_eq!(
            snap.len(),
            1,
            "the concurrent generate's busy must survive a stale clear"
        );
        assert_eq!(snap[0].key.as_deref(), Some(key));
        assert_eq!(snap[0].tone, "busy");
    }

    #[test]
    fn emitter_tick_upgrades_stale_busy_and_broadcasts_upgraded_wire_status() {
        // A Busy that outlives LAUNCH_TIMEOUT must be upgraded to Warning
        // in-place and broadcast as a live WireStatus update so the client sees
        // the change without a full reconnect.
        let (tx, mut rx) = broadcast::channel::<WireStatus>(16);
        let (clear_tx, _crx) = broadcast::channel::<Option<String>>(16);
        let (snap_tx, snap_rx) = watch::channel::<Vec<KeyedWireStatus>>(vec![]);
        let mut e = StatusEmitter {
            tx,
            clear_tx,
            snapshot_tx: snap_tx,
            controller: KeyedStatusController::with_clear_after(Duration::from_secs(6)),
            generations: std::collections::HashMap::new(),
        };
        // Drain the initial send so `rx` only sees the upgrade.
        let _ = e.send(WireStatus::keyed("launch", "busy", "Launching\u{2026}"));
        let _ = rx.try_recv(); // consume the live send

        // Advance past LAUNCH_TIMEOUT.
        let far_future = Instant::now() + LAUNCH_TIMEOUT + Duration::from_secs(1);
        e.tick(far_future);

        // The snapshot entry must have been upgraded to Warning.
        let snap = snap_rx.borrow().clone();
        assert_eq!(snap.len(), 1, "entry remains after busy→warning upgrade");
        assert_eq!(snap[0].tone, "warning");

        // The upgraded status must have been broadcast live.
        let upgraded = rx.try_recv().expect("upgraded status must be broadcast");
        assert_eq!(upgraded.key.as_deref(), Some("launch"));
        assert_eq!(upgraded.tone, "warning");
    }

    #[test]
    fn rebind_drift_is_false_for_identical_server_config() {
        let cfg = dux_core::config::ServerConfig::default();
        assert!(!server_rebind_settings_changed(&cfg, &cfg.clone()));
    }

    #[test]
    fn rebind_drift_detects_port_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.port += 1;
        assert!(server_rebind_settings_changed(&prev, &next));
    }

    #[test]
    fn rebind_drift_detects_tailscale_toggle() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.tailscale_enabled = !prev.tailscale_enabled;
        assert!(server_rebind_settings_changed(&prev, &next));
    }

    #[test]
    fn rebind_drift_detects_listen_addrs_change() {
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.listen_addrs.push("0.0.0.0:9000".to_string());
        assert!(server_rebind_settings_changed(&prev, &next));
    }

    #[test]
    fn rebind_drift_detects_max_websocket_connections_change() {
        // The /ws connection-cap semaphore is built once at startup, so changing
        // the cap must surface as a restart-needed warning like the other
        // startup-bound settings — not be silently swallowed by a live reload.
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.max_websocket_connections += 1;
        assert!(server_rebind_settings_changed(&prev, &next));
    }

    #[test]
    fn rebind_drift_detects_acme_field_changes() {
        let base = dux_core::config::ServerConfig::default();

        let mut enabled = base.clone();
        enabled.acme.enabled = !base.acme.enabled;
        assert!(server_rebind_settings_changed(&base, &enabled));

        let mut domains = base.clone();
        domains.acme.domains.push("example.com".to_string());
        assert!(server_rebind_settings_changed(&base, &domains));

        let mut email = base.clone();
        email.acme.email = "ops@example.com".to_string();
        assert!(server_rebind_settings_changed(&base, &email));

        let mut http_port = base.clone();
        http_port.acme.http_port += 1;
        assert!(server_rebind_settings_changed(&base, &http_port));

        let mut https_port = base.clone();
        https_port.acme.https_port += 1;
        assert!(server_rebind_settings_changed(&base, &https_port));

        let mut production = base.clone();
        production.acme.production = !base.acme.production;
        assert!(server_rebind_settings_changed(&base, &production));

        let mut cache_dir = base.clone();
        cache_dir.acme.cache_dir = Some("/tmp/acme".to_string());
        assert!(server_rebind_settings_changed(&base, &cache_dir));
    }

    #[test]
    fn rebind_drift_ignores_insecure_allow_remote() {
        // insecure_allow_remote is a startup GATE input, not a bound value; a
        // running listener cannot drift because of it, so it must not warn.
        let prev = dux_core::config::ServerConfig::default();
        let mut next = prev.clone();
        next.insecure_allow_remote = !prev.insecure_allow_remote;
        assert!(!server_rebind_settings_changed(&prev, &next));
    }
}
