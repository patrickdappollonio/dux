//! axum router + the WebSocket handlers (`/ws/events` and the per-PTY sockets)
//! bridging the browser to the engine actor. All data reads and actions go over
//! REST (`/api/v1/*`); the sockets carry only change events + status (events) and
//! terminal byte streams (PTY).
//!
//! ## Route structure
//!
//! dux is a trusted-local tool: there is no login gate. Every route is served
//! plainly: static assets, `/healthz`, all `/api/v1/*` reads and actions, and
//! every WS upgrade (`/ws/events` and the per-PTY sockets).
//!
//! The Origin check on every WS upgrade still runs (cross-site WebSocket
//! hijacking defense): a browser attaches the page's `Origin`, and we only allow
//! same-host origins. Non-browser clients (no `Origin`) are allowed — documented
//! tradeoff: a CLI/test client is trusted to not be a hijacked browser tab.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};

use dux_core::statusline::{KeyedWireStatus, StatusScope};

use crate::changes::ChangesService;
use crate::console::Console;
use crate::engine_actor::{EngineHandle, SpineChange, WorkspaceDoc};
use crate::event_bus::{self, Event, EventBus};
use crate::pty_owners::PtySizeOwners;

#[derive(Clone)]
pub struct AppState {
    pub engine: EngineHandle,
    /// The `dux server` terminal console. A real (stdout) console on the CLI
    /// serve paths; a [`Console::noop`] for the TUI flip (which owns the
    /// terminal) and every test that does not assert console output. WS handlers
    /// emit life events through it; the access middleware reads it too.
    pub console: Console,
    /// The `[server]` settings a config reload can move on a listener that is
    /// already bound: `access_log` and `search_index_max_files`. Seeded from
    /// [`RouterParams`] here and rewritten by the engine actor on every reload,
    /// so the routes read the current value per request rather than one frozen
    /// at bind time.
    pub live_limits: Arc<crate::engine_actor::LiveServerLimits>,
    /// Caps concurrent EVENTS WebSocket connections
    /// (`[server] max_websocket_events_connections`). This is the `/ws/events`
    /// status/changed-files stream (`ws_events_upgrade`). Each upgrade takes a
    /// permit before upgrading and holds it for the socket's lifetime; when none
    /// are free the upgrade is refused with HTTP 503. This class is sized and
    /// exhausted INDEPENDENTLY of the agent and terminal PTY classes. A cheap `Arc`
    /// clone so every request hits the same permit pool.
    pub ws_events_semaphore: Arc<tokio::sync::Semaphore>,
    /// Caps concurrent AGENT-PTY WebSocket connections
    /// (`[server] max_websocket_agent_connections`). The embedded-terminal stream
    /// for an agent session. Sized and exhausted INDEPENDENTLY of the events and
    /// terminal classes.
    pub ws_agent_semaphore: Arc<tokio::sync::Semaphore>,
    /// Caps concurrent TERMINAL-PTY WebSocket connections
    /// (`[server] max_websocket_terminal_connections`). The standalone
    /// scratch-terminal stream. Sized and exhausted INDEPENDENTLY of the events and
    /// agent classes.
    pub ws_terminal_semaphore: Arc<tokio::sync::Semaphore>,
    /// Caps concurrent EXTRA-TAB PTY WebSocket connections across all agents
    /// (`[server] max_websocket_tab_connections`). Tab sockets draw from THIS pool,
    /// not `ws_agent_semaphore`, so tabs can never starve the session-slot tab (agent) pool.
    pub ws_tab_semaphore: Arc<tokio::sync::Semaphore>,
    /// Per-agent fairness sub-quota (`[server] max_websocket_tabs_per_agent`) on
    /// top of `ws_tab_semaphore`: the count of live extra-tab sockets keyed by
    /// owning session id. `ws_tab_pty_upgrade` refuses a new tab socket for a
    /// session already at `max_ws_tabs_per_agent` BEFORE taking a tab-pool permit,
    /// so one agent's tabs cannot monopolize the shared tab pool. A [`TabWsGuard`]
    /// increments on connect and decrements on drop (every early return included).
    pub tab_ws_counts: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    /// The per-agent live-tab-socket ceiling (`[server] max_websocket_tabs_per_agent`).
    /// `0` permanently blocks all tab sockets (matching the WS-connection-cap family).
    pub max_ws_tabs_per_agent: u32,
    /// Bounds concurrent `/files/tree` directory listings across all sessions
    /// (`[server] tree_list_max_concurrency`). Protects the server's
    /// blocking-thread pool (`spawn_blocking`) from a burst of tree requests
    /// exhausting it and starving other blocking work. `None` when the
    /// configured value is `0` (unlimited): the route skips acquiring a
    /// permit entirely. A request beyond the limit WAITS for a free permit
    /// (`acquire().await`) rather than being rejected — unlike the
    /// `ws_*_semaphore` connection caps, this bounds a small, fast unit of
    /// background work, not a long-lived connection.
    pub tree_list_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// Bounds concurrent release-notes fetches (`GET /api/v1/release-notes`)
    /// (`[server] release_notes_max_concurrency`). Same shape and rationale as
    /// [`tree_list_semaphore`]: the fetch is a blocking HTTPS round trip on a
    /// `spawn_blocking` thread and every browser tab can ask for it, so a burst
    /// must not exhaust the blocking pool. `None` when the configured value is
    /// `0` (unlimited): the route skips acquiring a permit. A request beyond the
    /// limit WAITS (`acquire_owned().await`) rather than being rejected — and
    /// with the six-hour notes cache the waiter usually answers from cache.
    pub release_notes_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// Per-file size cap for a dropped file (`[server] file_drop_max_bytes`),
    /// applied to the upload route as an explicit body limit. `0` disables file
    /// drop entirely and the route refuses every upload.
    pub file_drop_max_bytes: usize,
    /// Bounds how many dropped-file uploads are in flight
    /// (`[server] file_drop_max_concurrency`). Unlike [`tree_list_semaphore`]
    /// this is never `None`: a configured `0` clamps to one permit, because the
    /// point of this bound is total buffered-upload MEMORY and "unlimited" would
    /// not bound it at all. The permit is taken in a LAYER around the handler,
    /// not inside it, because a request body is buffered in full before the
    /// handler's first line runs.
    pub file_drop_semaphore: Arc<tokio::sync::Semaphore>,
    /// The web-layer event bus: resource-change signals (`/ws/events`) plus the
    /// per-topic interest refcount that drives the changed-files poller.
    pub event_bus: Arc<EventBus>,
    /// The per-session changed-files cache + single-flight compute + poller behind
    /// `GET /api/v1/sessions/:id/changes` and the `session.changes` event. The git
    /// mutation handlers call `state.changes.invalidate(id)` after a successful
    /// stage/unstage/discard/commit/write so the pane refreshes immediately.
    pub changes: Arc<ChangesService>,
    /// The resource-monitor sampler + single-flight TTL cache behind
    /// `GET /api/v1/resources`, which the web Task Manager polls while open.
    pub resources: Arc<crate::resource_routes::ResourceService>,
    /// `Idempotency-Key -> created resource id` cache (TTL-bounded) so a retried
    /// `POST /api/v1/sessions` or `/projects` after a lost response returns the
    /// same resource instead of creating a duplicate worktree/project.
    pub idempotency: Arc<crate::rest_common::IdempotencyCache>,
    /// Per-PTY sizing ownership so two viewers of one PTY don't thrash its size.
    /// The most recently attached per-PTY socket owns sizing; a non-owner's resize
    /// frame is ignored (see [`PtySizeOwners`]).
    pty_size_owners: Arc<PtySizeOwners>,
    /// The per-PTY grid-change bus. Every applied resize is announced here and
    /// forwarded to every socket attached to that PTY as a `size` event frame,
    /// so a viewer learns the authoritative grid it is NOT rendering at (see
    /// [`crate::pty_sizes`]).
    pty_grid_bus: Arc<crate::pty_sizes::PtyGridBus>,
    /// The live-connection registry: every upgraded WebSocket (events + both PTY
    /// families) records its server-minted id and class here on connect and removes
    /// it on disconnect. Read by `scope_from_headers` to validate an inbound
    /// `X-Connection-Id` (unknown id → broadcast) and by `count` for a later task's
    /// per-class caps. A cheap `Arc` clone so every request/socket shares one map.
    pub connections: Arc<crate::rest_common::ConnectionRegistry>,
    /// This launch's pending first-load screen (welcome / what's-new), decided
    /// ONCE by the resolver task spawned in [`build_app`] and injected into the
    /// bootstrap document per request. See [`crate::first_load_routes`] for why
    /// it is not computed per request and why the version is stamped on
    /// dismissal rather than at startup.
    pub first_load: Arc<crate::first_load_routes::FirstLoadState>,
    /// This serve's live Tailscale-mode handle, or `None` when nothing is serving
    /// behind this router (tests, and any path with no serve loop). The route
    /// answers "the choice is saved and applies when a listener starts" for
    /// `None` rather than pretending the listener moved.
    pub tailscale_mode: Option<crate::serve_legs::TailscaleModeControl>,
    /// Whether this run was started with `--no-tailscale`. Projected into the
    /// bootstrap document so the Preferences row can say the flag outranks the
    /// saved value for as long as this run lasts.
    pub tailscale_forced_no: bool,
}

impl AppState {
    /// Whether some OTHER connection currently holds input on `pty_id`.
    ///
    /// This exists so the file-drop route can refuse a drop from a read-only
    /// viewer with a clear message instead of saving a file the browser will
    /// then be unable to paste. **It is a COURTESY, not the protection.** The
    /// only thing enforcing input authority is the websocket's own write check
    /// (see [`PtySizeOwners::may_write`]), which is why the upload route saves
    /// bytes and never writes to a terminal: a handler injecting text would walk
    /// straight past that gate, and hiding the drop target from non-owners is a
    /// courtesy too.
    ///
    /// An UNOWNED pty is not denied, matching `may_write`: the first write
    /// claims it.
    pub(crate) fn input_held_by_someone_else(&self, pty_id: &str, conn_id: u64) -> bool {
        matches!(
            self.pty_size_owners.owners.lock().unwrap().map.get(pty_id),
            Some(record) if record.conn_id != conn_id
        )
    }

    /// Hand input ownership of `pty_id` to `conn_id`, for tests that need the
    /// courtesy check above to have something to say.
    ///
    /// Test-only and narrow on purpose: the alternative was widening the
    /// `pty_size_owners` field to the whole crate so a route test could reach
    /// past `AppState`'s surface into a lock, which is a lot of new reach to buy
    /// one fixture. Ownership is otherwise only ever taken by a live terminal
    /// socket, which a `oneshot` router test has no way to open.
    #[cfg(test)]
    pub(crate) fn give_input_to(&self, pty_id: &str, conn_id: u64) {
        let _ = self.pty_size_owners.claim(pty_id, conn_id);
    }
}

/// Maximum size of a single inbound WebSocket MESSAGE (text or binary), down
/// from tungstenite's untuned 64 MiB default, so a client cannot push an
/// arbitrarily large message. It is NOT a total-memory cap (the theoretical
/// worst case is `REQ_CHANNEL_CAPACITY` queued messages of this size). In
/// practice in-flight memory stays far below that product: the engine drains the
/// whole request channel every tick, and link bandwidth caps how many large
/// messages can even arrive per tick. 16 MiB is far above any realistic terminal
/// paste, so legitimate input is never truncated.
///
/// A message is not a frame, and the difference matters when reading the tests.
/// A message may arrive as a continuation chain of frames, each separately
/// bounded by `max_frame_size`, which dux leaves at tungstenite's default. That
/// default is ALSO 16 MiB, so for an unfragmented payload the two limits are
/// indistinguishable and the frame cap is what fires. Only a fragmented message
/// can observe this constant, which is why
/// `a_fragmented_message_past_the_message_cap_is_refused` exists and why the
/// single-frame test beside it documents that it pins the library's default
/// rather than this number.
///
/// Public so the integration tests can name the same number the sockets are
/// configured with, rather than restating 16 MiB and silently passing if the two
/// ever drift.
pub const MAX_WS_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Upper bound (in characters) on a captured `User-Agent` before it is stamped on a
/// `pty.owner` handover. The raw header is attacker-controllable and re-broadcast to
/// every connected client on each ownership flip, so an unbounded value is an
/// amplification vector and would blow up the take-over modal title. Truncation is
/// char-safe (never byte slicing) to stay UTF-8 correct.
const MAX_CAPTURED_USER_AGENT_CHARS: usize = 200;

/// Read and length-bound the request `User-Agent` for the `pty.owner` handover.
/// Returns `None` when the header is absent or not valid UTF-8; otherwise truncates
/// to [`MAX_CAPTURED_USER_AGENT_CHARS`] using char-safe truncation.
fn captured_user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(MAX_CAPTURED_USER_AGENT_CHARS).collect())
}

/// Build the router. dux is trusted-local with no login gate, so every route is
/// plain. The single-argument entry the test harnesses and any caller use.
pub fn router(engine: EngineHandle) -> Router {
    build_app(engine, Router::new(), RouterParams::plain_http())
}

/// Wall-clock cadence of the per-socket liveness ping. Every upgraded socket
/// (events + both PTY families) sends a WebSocket Ping frame on this interval from
/// inside its own `select!` loop; the peer (browser or proxy) auto-responds with a
/// Pong at the protocol layer, so the ping both keeps an idle connection from being
/// reaped by a NAT/proxy and surfaces a dead peer.
///
/// LIVENESS APPROACH (deliberately the smallest correct one — see the task brief's
/// YAGNI note): this is a SEND-FAILURE reap, not a pong-deadline reap. A ping that
/// fails to send (the TCP send buffer has backed up against a dead/half-open peer,
/// or the socket is already closed) breaks the socket's loop, which drops the
/// connection-cap permit and the `ConnectionGuard` (deregistering the id), freeing
/// the slot. We do NOT track pong receipt against a grace window: doing so would
/// add per-socket pong-timestamp state for marginal benefit on a trusted,
/// single-tenant tool, and the brief explicitly permits the send-failure reap. The
/// reuse of each socket's existing per-socket `select!` loop keeps sinks out of the
/// registry and avoids any lock-across-await. Upgradeable to a true pong-deadline
/// reaper later if a half-open connection that still accepts buffered writes proves
/// to be a problem in practice.
const WS_LIVENESS_PING_PERIOD: std::time::Duration = std::time::Duration::from_secs(30);

/// Knobs for the serve paths.
#[derive(Clone)]
pub struct RouterParams {
    /// The console handler events (and the access middleware) emit through.
    /// Defaults to [`Console::noop`] so the flip and tests stay silent; the CLI
    /// serve paths replace it with a real stdout console via [`with_console`].
    pub console: Console,
    /// Whether the per-request access log is on (`[server] access_log`). Off by
    /// default; the CLI serve paths set it from config.
    pub access_log: bool,
    /// Cap on concurrent EVENTS `/ws/events` connections
    /// (`[server] max_websocket_events_connections`). Defaults to
    /// [`dux_core::config::DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS`]; the serve
    /// paths override it from config via [`with_max_websocket_connections`].
    pub max_websocket_events_connections: u32,
    /// Cap on concurrent AGENT-PTY WebSocket connections
    /// (`[server] max_websocket_agent_connections`). Defaults to
    /// [`dux_core::config::DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS`].
    pub max_websocket_agent_connections: u32,
    /// Cap on concurrent TERMINAL-PTY WebSocket connections
    /// (`[server] max_websocket_terminal_connections`). Defaults to
    /// [`dux_core::config::DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS`].
    pub max_websocket_terminal_connections: u32,
    /// Cap on concurrent extra-tab PTY WebSocket connections across all agents
    /// (`[server] max_websocket_tab_connections`). Defaults to
    /// [`dux_core::config::DEFAULT_MAX_WEBSOCKET_TAB_CONNECTIONS`].
    pub max_websocket_tab_connections: u32,
    /// Per-agent live-tab-socket sub-quota (`[server] max_websocket_tabs_per_agent`).
    /// Defaults to [`dux_core::config::DEFAULT_MAX_WEBSOCKET_TABS_PER_AGENT`].
    pub max_websocket_tabs_per_agent: u32,
    /// Cap on the editor's file-search index flat walk
    /// (`[server] search_index_max_files`). Defaults to
    /// [`dux_core::config::DEFAULT_SEARCH_INDEX_MAX_FILES`]; the serve paths
    /// override it from config via [`with_search_index_max_files`].
    pub search_index_max_files: usize,
    /// Deadline on one of a PTY socket's OPENING sends, in seconds
    /// (`[server] pty_send_timeout_seconds`). Defaults to
    /// [`dux_core::config::DEFAULT_PTY_SEND_TIMEOUT_SECONDS`]; the serve paths
    /// override it from config via [`ServeParams::with_pty_send_timeout_seconds`].
    pub pty_send_timeout_seconds: u32,
    /// Cap on concurrent `/files/tree` directory listings
    /// (`[server] tree_list_max_concurrency`). Defaults to
    /// [`dux_core::config::DEFAULT_TREE_LIST_MAX_CONCURRENCY`]; the serve
    /// paths override it from config via
    /// [`with_tree_list_max_concurrency`]. `0` means unlimited.
    pub tree_list_max_concurrency: u32,
    /// Cap on concurrent release-notes fetches
    /// (`[server] release_notes_max_concurrency`). Defaults to
    /// [`dux_core::config::DEFAULT_RELEASE_NOTES_MAX_CONCURRENCY`]; the serve
    /// paths override it from config via
    /// [`with_release_notes_max_concurrency`]. `0` means unlimited.
    pub release_notes_max_concurrency: u32,
    /// Per-file size cap for a dropped file (`[server] file_drop_max_bytes`).
    /// Defaults to [`dux_core::config::DEFAULT_FILE_DROP_MAX_BYTES`]; the serve
    /// paths override it from config via [`with_file_drop_limits`]. `0` disables
    /// file drop.
    pub file_drop_max_bytes: usize,
    /// Cap on concurrent dropped-file uploads
    /// (`[server] file_drop_max_concurrency`). Defaults to
    /// [`dux_core::config::DEFAULT_FILE_DROP_MAX_CONCURRENCY`]; `0` clamps to 1.
    pub file_drop_max_concurrency: u32,
    /// The IPs the server actually bound to. When non-empty, `build_app` wraps
    /// the router with the Host allowlist (DNS-rebinding defense). An empty vec
    /// disables the guard; used by tests that do not exercise the host guard.
    pub bound_ips: Vec<IpAddr>,
    /// Operator-configured hostnames from `[server] allowed_hosts`. Normalized
    /// (lowercased, port-stripped) inside the allowlist; raw strings here. Only
    /// meaningful when the host guard is active (see [`bound_ips`]).
    pub configured_hosts: Vec<String>,
    /// Whether the allowlist accepts IP literals in Tailscale's own ranges
    /// (`[server] tailscale` is not `"no"`). Only meaningful when the host guard
    /// is active (see [`bound_ips`]).
    pub tailscale_host_literals: bool,
    /// The serve's Tailscale-mode cell, when a serve is behind this router. The
    /// Host guard reads rule 5 from it per request, so a mode change applied
    /// while dux serves moves the guard with the listener. `None` in tests and on
    /// any path with no live mode control.
    pub live_tailscale_host_literals: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// The handle the Tailscale-mode route changes `[server] tailscale` through
    /// while dux serves. `None` on any path with no serve loop behind it, and in
    /// tests.
    pub tailscale_mode_control: Option<crate::serve_legs::TailscaleModeControl>,
    /// Whether this run was started with `--no-tailscale`.
    pub tailscale_forced_no: bool,
    /// Base URL for release-notes fetches. Defaults to
    /// `dux_core::urls::GITHUB_API_BASE`; overridden only by tests (see
    /// [`RouterParams::with_release_notes_api_base`]).
    pub release_notes_api_base: String,
    /// A slot for `build_app` to leave this serve's ownership publisher in, so a
    /// caller outside the router can announce ownership changes on the same two
    /// buses the socket handlers use.
    ///
    /// An out-parameter rather than a return value because the router swallows
    /// the app state whole: both buses are born inside `build_app` and there is
    /// no other way back to them. `None` for every serve path but the background
    /// one, which is the only one with a second surface to announce for.
    pub(crate) ownership_publisher:
        Option<Arc<std::sync::OnceLock<crate::ownership_publish::OwnershipPublisher>>>,
    /// A counter for the connection registry to keep the live browser-tab count in.
    ///
    /// An in-parameter rather than an out one, unlike the publisher above: the
    /// caller can make an `AtomicUsize` itself, and it needs the handle before
    /// anything connects. `None` for every serve path but the background one,
    /// which is the only one with a terminal UI beside it to show the count on.
    pub(crate) connections_gauge: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl RouterParams {
    /// Plain-HTTP defaults: a no-op console, no access log, host guard off.
    /// Used by the loopback/Tailscale/proxy serve paths and every test harness;
    /// the CLI paths layer a real console and the allowlist on with
    /// [`with_console`] and [`with_host_allowlist`].
    pub fn plain_http() -> Self {
        Self {
            console: Console::noop(),
            access_log: false,
            max_websocket_events_connections:
                dux_core::config::DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS,
            max_websocket_agent_connections:
                dux_core::config::DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS,
            max_websocket_terminal_connections:
                dux_core::config::DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS,
            max_websocket_tab_connections: dux_core::config::DEFAULT_MAX_WEBSOCKET_TAB_CONNECTIONS,
            max_websocket_tabs_per_agent: dux_core::config::DEFAULT_MAX_WEBSOCKET_TABS_PER_AGENT,
            search_index_max_files: dux_core::config::DEFAULT_SEARCH_INDEX_MAX_FILES,
            pty_send_timeout_seconds: dux_core::config::DEFAULT_PTY_SEND_TIMEOUT_SECONDS,
            tree_list_max_concurrency: dux_core::config::DEFAULT_TREE_LIST_MAX_CONCURRENCY,
            release_notes_max_concurrency: dux_core::config::DEFAULT_RELEASE_NOTES_MAX_CONCURRENCY,
            file_drop_max_bytes: dux_core::config::DEFAULT_FILE_DROP_MAX_BYTES,
            file_drop_max_concurrency: dux_core::config::DEFAULT_FILE_DROP_MAX_CONCURRENCY,
            bound_ips: Vec::new(),
            configured_hosts: Vec::new(),
            tailscale_host_literals: false,
            live_tailscale_host_literals: None,
            tailscale_mode_control: None,
            tailscale_forced_no: false,
            release_notes_api_base: dux_core::urls::GITHUB_API_BASE.to_string(),
            ownership_publisher: None,
            connections_gauge: None,
        }
    }

    /// Ask `build_app`'s connection registry to keep its live browser-tab count in
    /// `gauge`, so the terminal UI's serving chip can read it.
    ///
    /// Only the background serve calls this, for the same reason as
    /// [`Self::with_ownership_publisher`]: it is the one path with a second surface
    /// that has somewhere to show the number.
    pub(crate) fn with_connections_gauge(
        mut self,
        gauge: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        self.connections_gauge = Some(gauge);
        self
    }

    /// Ask `build_app` to leave this serve's ownership publisher in `slot`.
    ///
    /// Only the background serve calls this: it is the one path with a second
    /// surface (a live terminal UI) that can claim a pty and therefore has
    /// something to announce.
    pub(crate) fn with_ownership_publisher(
        mut self,
        slot: Arc<std::sync::OnceLock<crate::ownership_publish::OwnershipPublisher>>,
    ) -> Self {
        self.ownership_publisher = Some(slot);
        self
    }

    /// Point release-notes fetches at `base` instead of the real GitHub API.
    /// Exists for tests: no test may contact api.github.com, so the integration
    /// suite serves a canned release payload from a local listener and passes its
    /// base here. Production never calls this and keeps
    /// `dux_core::urls::GITHUB_API_BASE`.
    pub fn with_release_notes_api_base(mut self, base: impl Into<String>) -> Self {
        self.release_notes_api_base = base.into();
        self
    }

    /// Set the file-search index cap from `[server] search_index_max_files`.
    /// The serve paths call this so the configured value (not just the default)
    /// bounds the flat walk behind `/files/list`.
    pub fn with_search_index_max_files(mut self, max_files: usize) -> Self {
        self.search_index_max_files = max_files;
        self
    }

    /// Set the opening-send deadline from `[server] pty_send_timeout_seconds`.
    /// The serve paths call this so the configured value (not just the default)
    /// bounds the handshake and the scrollback replay.
    pub fn with_pty_send_timeout_seconds(mut self, seconds: u32) -> Self {
        self.pty_send_timeout_seconds = seconds;
        self
    }

    /// Set the concurrent tree-listing cap from `[server]
    /// tree_list_max_concurrency`. The serve paths call this so the
    /// configured value (not just the default) bounds `/files/tree`.
    pub fn with_tree_list_max_concurrency(mut self, max_concurrency: u32) -> Self {
        self.tree_list_max_concurrency = max_concurrency;
        self
    }

    /// Set the concurrent release-notes-fetch cap from `[server]
    /// release_notes_max_concurrency`. The serve paths call this so the
    /// configured value (not just the default) bounds `/api/v1/release-notes`.
    pub fn with_release_notes_max_concurrency(mut self, max_concurrency: u32) -> Self {
        self.release_notes_max_concurrency = max_concurrency;
        self
    }

    /// Set the file-drop size and concurrency caps from `[server]
    /// file_drop_max_bytes` / `file_drop_max_concurrency`. The serve paths call
    /// this so the configured values (not just the defaults) bound
    /// `/api/v1/file-drop`. Both are read here, at startup, which is why the
    /// documentation says changing either needs a restart.
    pub fn with_file_drop_limits(mut self, max_bytes: usize, max_concurrency: u32) -> Self {
        self.file_drop_max_bytes = max_bytes;
        self.file_drop_max_concurrency = max_concurrency;
        self
    }

    /// Attach a real console + the access-log toggle. The CLI serve paths call
    /// this so handler events and the access middleware reach stdout; the flip
    /// leaves the no-op console in place.
    pub fn with_console(mut self, console: Console, access_log: bool) -> Self {
        self.console = console;
        self.access_log = access_log;
        self
    }

    /// Set the per-class concurrent-connection caps from the three
    /// `[server] max_websocket_*_connections` settings. The serve paths call this
    /// so the configured values (not just the defaults) bound live sockets, each
    /// class independently.
    pub fn with_max_websocket_connections(
        mut self,
        events: u32,
        agent: u32,
        terminal: u32,
        tab: u32,
        tab_per_agent: u32,
    ) -> Self {
        self.max_websocket_events_connections = events;
        self.max_websocket_agent_connections = agent;
        self.max_websocket_terminal_connections = terminal;
        self.max_websocket_tab_connections = tab;
        self.max_websocket_tabs_per_agent = tab_per_agent;
        self
    }

    /// Activate the Host allowlist (DNS-rebinding defense). `bound_ips` is the
    /// set of IPs the server actually bound to (derived from the bound listeners
    /// in `lib.rs`); `configured` is the raw `[server] allowed_hosts` list. When
    /// this is set, `build_app` wraps the whole router with the host allowlist
    /// middleware, which runs OUTSIDE the access-log layer so foreign-Host probes
    /// are not access-logged.
    /// `tailscale_host_literals` activates the allowlist's structural
    /// Tailscale-range rule (rule 5). It comes from the serve MODE, not from what
    /// bound: on `auto` the Tailscale leg comes and goes behind this one
    /// immutable allowlist, and a rule derived from the startup bind set would
    /// 403 every tailnet device the moment the leg was re-bound.
    pub fn with_host_allowlist(
        mut self,
        bound_ips: Vec<IpAddr>,
        configured: Vec<String>,
        tailscale_host_literals: bool,
    ) -> Self {
        self.bound_ips = bound_ips;
        self.configured_hosts = configured;
        self.tailscale_host_literals = tailscale_host_literals;
        self
    }

    /// Set rule 5's constructed value on its own, for a caller whose EFFECTIVE
    /// mode differs from the configured one that [`crate::router_params`]
    /// derived (a run started with `--no-tailscale`).
    pub fn with_tailscale_host_literals(mut self, allowed: bool) -> Self {
        self.tailscale_host_literals = allowed;
        self
    }

    /// Give the routes the handle that changes `[server] tailscale` while dux
    /// serves. Absent on any path with no serve loop behind it, which is what
    /// makes the route answer "nothing is serving" rather than hanging.
    pub fn with_tailscale_mode_control(
        mut self,
        control: crate::serve_legs::TailscaleModeControl,
        forced_no: bool,
    ) -> Self {
        self.tailscale_mode_control = Some(control);
        self.tailscale_forced_no = forced_no;
        self
    }

    /// Let the Host guard read rule 5 from the serve's live Tailscale-mode cell,
    /// so a mode change applied while dux serves moves the guard with it.
    pub fn with_live_tailscale_host_literals(
        mut self,
        cell: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.live_tailscale_host_literals = Some(cell);
        self
    }
}

/// Build the dux web router. dux is trusted-local: there is no login gate, so
/// every route is served plainly. `extra_gated` is merged into the router as-is
/// (a test seam for an injected probe route); production callers pass an empty
/// router.
///
/// ## Middleware stack (outermost to innermost)
///
/// 1. **Host allowlist** (DNS-rebinding defense): when `params.bound_ips` is
///    non-empty, the outermost layer rejects any request whose `Host` header is
///    not in the allowlist with `403`. Foreign-Host probes are rejected before
///    the access log runs, so they are never logged.
/// 2. **Access log**: logs every request (method, path, status, latency) when
///    the console is active and `access_log` is on. Sees the final status
///    produced by every inner layer, including the REST mutation check's 403.
/// 3. **REST mutation origin check**: rejects cross-origin POST/PATCH/PUT/DELETE
///    requests (cross-site request forgery defense). A missing `Origin` (curl,
///    CLI clients) is allowed; a present but unparseable `Origin` (including the
///    literal `"null"` from sandboxed iframes) is treated as a mismatch and
///    rejected. Shares the `same_origin_allowed` helper with the WS upgrade
///    handlers so REST and WS use one authority-comparison implementation.
/// 4. **Handlers**: the actual route logic.
pub fn build_app(
    engine: EngineHandle,
    extra_gated: Router<AppState>,
    params: RouterParams,
) -> Router {
    // A zero cap is a valid-but-drastic per-class setting ("refuse all new
    // connections of this class until restart"). Warn loudly at startup so an
    // accidental 0 isn't a silent lock-out: every upgrade of that class would 503
    // with no other clue (explicit failure over silence). The events class at 0
    // makes the web UI unreachable; the PTY classes at 0 block only their stream.
    if params.max_websocket_events_connections == 0 {
        dux_core::logger::warn(
            "[server] max_websocket_events_connections = 0: every events WebSocket \
             upgrade will be refused with HTTP 503 and the web UI will be unreachable \
             until the server restarts",
        );
    }
    if params.max_websocket_agent_connections == 0 {
        dux_core::logger::warn(
            "[server] max_websocket_agent_connections = 0: every agent-PTY WebSocket \
             upgrade will be refused with HTTP 503 until the server restarts",
        );
    }
    if params.max_websocket_terminal_connections == 0 {
        dux_core::logger::warn(
            "[server] max_websocket_terminal_connections = 0: every terminal-PTY \
             WebSocket upgrade will be refused with HTTP 503 until the server restarts",
        );
    }
    if params.max_websocket_tab_connections == 0 {
        dux_core::logger::warn(
            "[server] max_websocket_tab_connections = 0: every extra-tab PTY \
             WebSocket upgrade will be refused with HTTP 503 until the server restarts",
        );
    }
    if params.max_websocket_tabs_per_agent == 0 {
        dux_core::logger::warn(
            "[server] max_websocket_tabs_per_agent = 0: every extra-tab PTY \
             WebSocket upgrade will be refused with HTTP 503 until the server restarts",
        );
    }
    // The event bus and changed-files service are web-layer concerns built here.
    // `ChangesService::new` spawns its supervised poller, so this must run inside a
    // tokio runtime context -- the CLI serve paths build inside `block_on`, and the
    // flip wraps its `build_app` in `runtime.enter()` (see `serve_with_engine`).
    let event_bus = Arc::new(EventBus::new());
    let changes = ChangesService::new(engine.clone(), Arc::clone(&event_bus));
    let resources = crate::resource_routes::ResourceService::new(engine.clone());
    // Config-reload -> `config.changed` forwarder. The engine actor fires `()` on
    // its config-reload broadcast after a successful reload; we turn each into a
    // coarse `config.changed` event so clients on the `config` topic refetch
    // `/api/v1/bootstrap`. The engine thread is spawned before this builder runs,
    // so the bus cannot live on the engine; the forwarder bridges the two. Runs for
    // the server lifetime (like the ChangesService poller) and exits when the engine
    // is gone. Requires a tokio runtime context, which every `build_app` caller
    // provides (the CLI serve paths build inside `block_on`; the flip enters it).
    spawn_config_changed_forwarder(engine.subscribe_config_reloads(), Arc::clone(&event_bus));
    // Spine-change -> `projects.changed` / `sessions.changed` forwarder. The engine
    // loop fires a `SpineChange` whenever the projected projects- or
    // sessions+sidebar-portion changes; we turn each into the matching coarse event
    // so clients on the `projects` / `sessions` topics know the document moved.
    // Those clients are also PUSHED the new document itself, on a separate watch
    // channel read by the socket; this signal is what an older page refetches on.
    // Same lifetime/teardown story as the config forwarder above.
    spawn_spine_changed_forwarder(engine.subscribe_spine_changes(), Arc::clone(&event_bus));
    // Run the first-load gate ONCE for this launch, off every request path. The
    // resolver parks the screen (if any) in this state and emits `config.changed`
    // so already-connected clients refetch bootstrap and find it; clients that
    // connect later just read it out of their first bootstrap. Same lifetime and
    // runtime-context story as the forwarders above.
    let first_load = Arc::new(crate::first_load_routes::FirstLoadState::new(
        params.release_notes_api_base,
    ));
    crate::first_load_routes::spawn_first_load_resolver(
        engine.clone(),
        Arc::clone(&event_bus),
        Arc::clone(&first_load),
    );
    // Clone the shared input-ownership registry out of the handle before the
    // handle itself moves into the state literal below.
    let engine_pty_owners = engine.pty_input_owners();
    // Both buses exist by now, so a caller that asked for the ownership publisher
    // can have it. Built here rather than returned, because the state literal
    // below moves everything into the router. `set` rather than an assignment:
    // one publisher per serve, and a second `build_app` on the same slot is a bug
    // worth ignoring rather than one worth overwriting silently.
    let pty_grid_bus = Arc::new(crate::pty_sizes::PtyGridBus::default());
    if let Some(slot) = params.ownership_publisher.as_ref() {
        let _ = slot.set(crate::ownership_publish::OwnershipPublisher::new(
            Arc::clone(&event_bus),
            Arc::clone(&pty_grid_bus),
        ));
    }
    // The router's bind-time values seed the shared cells; a later reload
    // overwrites them through the actor. Seeded from the params rather than the
    // engine's config because a serve path or a test may pass either.
    let live_limits = engine.live_limits();
    live_limits.set_access_log(params.access_log);
    live_limits.set_search_index_max_files(params.search_index_max_files);
    live_limits.set_pty_send_timeout_seconds(params.pty_send_timeout_seconds as usize);
    let state = AppState {
        engine,
        console: params.console,
        live_limits,
        ws_events_semaphore: Arc::new(tokio::sync::Semaphore::new(
            params.max_websocket_events_connections as usize,
        )),
        ws_agent_semaphore: Arc::new(tokio::sync::Semaphore::new(
            params.max_websocket_agent_connections as usize,
        )),
        ws_terminal_semaphore: Arc::new(tokio::sync::Semaphore::new(
            params.max_websocket_terminal_connections as usize,
        )),
        ws_tab_semaphore: Arc::new(tokio::sync::Semaphore::new(
            params.max_websocket_tab_connections as usize,
        )),
        tab_ws_counts: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        max_ws_tabs_per_agent: params.max_websocket_tabs_per_agent,
        // `0` means unlimited: skip the semaphore entirely rather than build a
        // zero-permit one, which would block every request forever (the
        // opposite of the ws_*_semaphore "0 = block all" convention).
        tree_list_semaphore: if params.tree_list_max_concurrency == 0 {
            None
        } else {
            Some(Arc::new(tokio::sync::Semaphore::new(
                params.tree_list_max_concurrency as usize,
            )))
        },
        // Same `0 = unlimited` convention as `tree_list_semaphore` above.
        release_notes_semaphore: if params.release_notes_max_concurrency == 0 {
            None
        } else {
            Some(Arc::new(tokio::sync::Semaphore::new(
                params.release_notes_max_concurrency as usize,
            )))
        },
        file_drop_max_bytes: params.file_drop_max_bytes,
        // Deliberately NOT the `0 = unlimited` convention of the two semaphores
        // above. This one bounds how much upload is held in MEMORY at once, so
        // "unlimited" would defeat its only purpose; `0` clamps to one permit
        // instead. Switching file drop off is what `file_drop_max_bytes = 0` is
        // for.
        file_drop_semaphore: Arc::new(tokio::sync::Semaphore::new(
            params.file_drop_max_concurrency.max(1) as usize,
        )),
        event_bus,
        changes,
        resources,
        idempotency: Arc::new(crate::rest_common::IdempotencyCache::new()),
        // Shared with the engine actor loop (see `build_actor_channels`), which
        // overlays the owner map onto the spine so every client learns which
        // connection is driving each agent PTY without attaching to it.
        pty_size_owners: engine_pty_owners,
        pty_grid_bus: Arc::clone(&pty_grid_bus),
        connections: Arc::new(match params.connections_gauge {
            Some(gauge) => crate::rest_common::ConnectionRegistry::with_events_gauge(gauge),
            None => crate::rest_common::ConnectionRegistry::new(),
        }),
        first_load,
        tailscale_mode: params.tailscale_mode_control.clone(),
        tailscale_forced_no: params.tailscale_forced_no,
    };

    // Every route is served plainly (trusted-local: no login gate). `extra_gated`
    // is merged as-is so a test can inject a probe route.
    let router = Router::new()
        .route("/ws/events", get(ws_events_upgrade))
        // Nested per-PTY byte-stream sockets. One socket per attached PTY: the
        // agent session's main provider PTY and a companion terminal's PTY. Each
        // replicates the WS protections in its upgrade handler.
        .route("/ws/sessions/{id}/pty", get(ws_session_pty_upgrade))
        .route(
            "/ws/sessions/{id}/terminals/{tid}/pty",
            get(ws_terminal_pty_upgrade),
        )
        .route(
            "/ws/projects/{id}/terminals/{tid}/pty",
            get(ws_project_terminal_pty_upgrade),
        )
        .route(
            "/ws/terminals/{tid}/pty",
            get(ws_standalone_terminal_pty_upgrade),
        )
        .route("/ws/sessions/{id}/tabs/{tab}/pty", get(ws_tab_pty_upgrade))
        .merge(crate::git_routes::routes())
        .merge(crate::file_routes::routes())
        .merge(crate::changes_routes::routes())
        .merge(crate::resource_routes::routes())
        .merge(crate::bootstrap_routes::routes())
        .merge(crate::build_routes::routes())
        .merge(crate::workspace_routes::routes())
        .merge(crate::session_actions::routes())
        .merge(crate::project_actions::routes())
        .merge(crate::project_reads::routes())
        .merge(crate::startup_logs::routes())
        .merge(crate::terminal_actions::routes())
        .merge(crate::file_drop_routes::routes(&state))
        .merge(crate::tab_actions::routes())
        .merge(crate::browse_routes::routes())
        .merge(crate::config_routes::routes())
        .merge(crate::first_load_routes::routes())
        .merge(extra_gated)
        .route("/healthz", get(|| async { "ok" }))
        .fallback(crate::web_assets::static_handler)
        // REST mutation same-origin check (cross-site request forgery defense).
        // Rejects POST/PATCH/PUT/DELETE when an `Origin` header is present but
        // its `host:port` authority does not match the `Host` header. A missing
        // `Origin` (curl, CLI clients) is allowed. Shares `same_origin_allowed`
        // with the WS upgrade handlers for one authority-comparison path.
        // Sits INSIDE the access-log layer so 403s are access-logged.
        .layer(middleware::from_fn(rest_mutation_origin_check))
        // The access log is the OUTERMOST layer OF THIS inner app, so it sees the
        // final status every layer it wraps produced (including the mutation 403).
        // It is gated inside on `access_log && console.is_active`, so the flip
        // and disabled-console paths pay nothing. Stamped via
        // `from_fn_with_state` so it reads the console/toggle off `AppState`.
        //
        // The host allowlist (see below) is applied OUTSIDE this layer, so
        // foreign-Host probes are rejected before reaching the access log.
        .layer(middleware::from_fn_with_state(state.clone(), access_log))
        .with_state(state);

    // Host allowlist (DNS-rebinding defense): outermost layer so it runs before
    // the access log. Active when bound_ips is non-empty; tests that do not
    // exercise the host guard leave bound_ips empty, keeping the guard off.
    if !params.bound_ips.is_empty() || !params.configured_hosts.is_empty() {
        crate::host_guard::host_allowlist_layer(
            router,
            params.bound_ips,
            params.configured_hosts,
            params.tailscale_host_literals,
            params.live_tailscale_host_literals,
        )
    } else {
        router
    }
}

/// Per-request access-log middleware for the main app: print
/// `method path status latencyms` to the console after the inner stack produces a
/// response. Reads the console + toggle off [`AppState`] and delegates to the
/// shared [`log_request`] core.
async fn access_log(State(state): State<AppState>, request: Request, next: Next) -> Response {
    log_request(
        &state.console,
        state.live_limits.access_log(),
        request,
        next,
    )
    .await
}

/// The shared access-log core. CONSOLE-ONLY (never `dux.log` — piping
/// `dux server`'s stdout IS the access log). Skips `/healthz` so a health checker
/// does not flood the log, and is gated on `access_log && console.is_active()` so
/// the flip/disabled paths emit nothing.
///
/// The path is printed WITHOUT its query string. Query parameters can carry
/// sensitive values — `GET /api/v1/sessions/<id>/files/raw?path=…` puts a
/// worktree-relative filesystem path in the query — and this log is the
/// `dux server` stdout an operator may forward to a file or aggregator, so the
/// query is dropped to avoid leaking secrets. The session id is an opaque `:id`
/// path segment (not a query parameter) and so still appears in the logged path.
async fn log_request(
    console: &Console,
    access_log: bool,
    request: Request,
    next: Next,
) -> Response {
    // Check the cheap gates BEFORE allocating anything: a disabled access log or a
    // no-op console pays nothing per request. /healthz is intentionally never
    // logged (probe noise) — compared against the borrowed path, no allocation.
    let log = access_log && console.is_active() && request.uri().path() != "/healthz";
    if !log {
        return next.run(request).await;
    }
    let method = request.method().as_str().to_string();
    // Log the PATH ONLY — never the query string. Query params can carry secrets
    // (e.g. /api/v1/sessions/<id>/files/raw?path=…), and this log is stdout an
    // operator may persist, so dropping the query avoids leaking them. The session
    // id is an opaque path segment now, so it still appears in the logged path.
    let path = request.uri().path().to_string();
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    let latency_ms = started.elapsed().as_millis();
    console.access(&method, &path, response.status().as_u16(), latency_ms);
    response
}

/// Whether a WebSocket upgrade passes the same-host Origin check (cross-site
/// WebSocket hijacking defense). `true` when the request carries no `Origin`
/// (non-browser clients — CLIs, tests, native apps — don't send one, and the
/// tradeoff is documented) or when the `Origin`'s `host[:port]` matches the
/// `Host` header. `false` for a present-but-mismatched `Origin`. Browsers always
/// send `Origin` for WS, so this only ever rejects a genuine cross-site attempt.
// DNS-rebinding defense: the same-origin check below trusts the request's own
// `Host` header, so on its own it does not stop a rebinding attacker who points a
// controlled hostname at this server's IP (the browser then sends a matching
// Origin/Host pair). The host allowlist (see `host_guard::host_allowlist_layer`)
// runs AHEAD of the whole app on every serve path and pins the accepted `Host`
// values to loopback, the addresses dux actually bound, and any configured
// `allowed_hosts`, returning 403 for a mismatched Host and closing that gap. This
// same-origin check remains the WS-specific defense layered on top.
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
///
/// NOTE: the scheme is intentionally dropped, so the comparison in
/// [`same_origin_allowed`] is authority-only -- it does NOT distinguish an
/// `http://` Origin from an `https://` one for the same host. A cross-protocol
/// upgrade is not blocked here; browsers reject it via mixed-content policy, and
/// on the TLS path the host allowlist is the complementary layer.
///
/// Returns `None` for the literal `"null"` (sent by sandboxed iframes / `data:`
/// documents) and for any value without a `"://"` scheme separator, so callers
/// can treat an unparseable Origin as a cross-origin mismatch rather than
/// falling through to the no-Origin allow path.
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

/// Middleware: reject cross-origin REST mutations (cross-site request forgery
/// defense). Applies to POST, PATCH, PUT, and DELETE only; GET/HEAD/OPTIONS
/// pass through unconditionally.
///
/// - `Origin` absent (curl, CLI clients, server-to-server): allowed. Non-browser
///   clients do not send `Origin`, and the tradeoff is documented.
/// - `Origin` present and authority matches `Host` (same-origin browser): allowed.
/// - `Origin: null` (sandboxed iframe / `data:` document): `origin_host` returns
///   `None` (no `"://"` separator), which `same_origin_allowed` treats as a
///   mismatch -- rejected with 403. Do NOT fall through to the no-Origin allow
///   path when the value is present but unparseable.
/// - `Origin` present and authority does not match `Host`: rejected with 403.
///
/// Shares `same_origin_allowed` with the WS upgrade handlers so one authority-
/// comparison path serves both.
async fn rest_mutation_origin_check(request: Request, next: Next) -> Response {
    use axum::http::Method;
    let is_mutation = matches!(
        *request.method(),
        Method::POST | Method::PATCH | Method::PUT | Method::DELETE
    );
    if is_mutation && !same_origin_allowed(request.headers()) {
        return (StatusCode::FORBIDDEN, "cross-origin request rejected").into_response();
    }
    next.run(request).await
}

type SharedSink = Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WebSocket, Message>>>;

/// How long the forwarder's blocking reader parks per `recv_timeout` before
/// re-checking `shutdown`. Bounds the worst-case time a forwarder lingers after
/// a teardown begins, so the tokio blocking pool never wedges runtime shutdown.
const FORWARDER_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Forward std-mpsc PTY bytes into the socket as binary frames, off the async runtime.
///
/// Returns the async pump task's [`JoinHandle`]. Aborting it (or letting it end on a closed socket)
/// drops `async_rx`, which closes the bounded `tx`. The blocking reader then ends either via a failed
/// `blocking_send` on the next chunk OR, against a quiet PTY with no further output, via the
/// `tx.is_closed()` check in its `recv_timeout` timeout arm within one `FORWARDER_POLL` window.
/// Abort alone is NOT sufficient when the PTY is quiet — without the `is_closed` poll the blocking
/// task would loop forever. Once it ends it drops its std `Receiver`, so the owning `PtyClient`
/// prunes that stale subscriber on its next read.
///
/// The blocking reader parks on a bounded `recv_timeout` rather than `recv` so it can also exit on
/// `shutdown`: the std-mpsc `Sender` lives in the `PtyClient` reader thread and, on a ReturnToTui
/// flip, the engine (and thus that `Sender`) stays alive, so `recv` would never return Disconnected
/// and would wedge the tokio blocking pool — hanging the runtime teardown. Polling `shutdown` every
/// `FORWARDER_POLL` lets the task exit within one window of any teardown even with the engine alive.
///
/// The same timeout arm also checks `tx.is_closed()`: when the downstream socket closes against a
/// QUIET PTY (the async forwarder task ends and drops `async_rx`, but no further PTY output arrives
/// to make `blocking_send` observe the closure), polling `shutdown` alone would never fire and the
/// blocking task would loop forever, leaking a blocking-pool thread per focus-switch/disconnect.
/// Breaking on `is_closed` ends the blocking reader within one poll window of the socket dropping,
/// which in turn drops the std `Receiver` so the owning `PtyClient` prunes the stale subscriber.
/// WebSocket close code (application-private range 4000-4999, so it can never
/// collide with a protocol close code) the server sends on a PTY socket when the
/// provider is not available to attach to — it failed to launch (e.g. the CLI is
/// not on PATH) or its process has exited/crashed. It tells the client NOT to
/// auto-retry: a re-subscribe would just relaunch the doomed provider, so the
/// client stops and surfaces a Reconnect affordance instead of looping. Must
/// match `PROVIDER_UNAVAILABLE_CLOSE` in `crates/dux-web/web/src/lib/ptySocket.ts`.
const PROVIDER_GONE_CLOSE_CODE: u16 = 4001;

/// The "provider unavailable, do not retry" close message (see
/// [`PROVIDER_GONE_CLOSE_CODE`]).
fn provider_gone_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: PROVIDER_GONE_CLOSE_CODE,
        reason: "provider unavailable".into(),
    }))
}

/// The close to send when a PTY forwarder ends. During server shutdown it is a
/// plain close so the client reconnects once the server returns; otherwise the
/// forwarder ended because the PTY was torn down (the provider crashed/exited, or
/// a tab/agent was closed), which is a provider-gone close so the client does not
/// relaunch it.
fn forwarder_end_close(shutting_down: bool) -> Message {
    if shutting_down {
        Message::Close(None)
    } else {
        provider_gone_close()
    }
}

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
                    if shutdown.load(std::sync::atomic::Ordering::SeqCst) || tx.is_closed() {
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

/// Acquire a connection-cap permit before a WS upgrade, from the per-class
/// `semaphore` the caller passes (events, agent-PTY, or terminal-PTY). `None`
/// means that class's cap is exhausted (the caller responds 503); a refusal is
/// logged here with `route` and `cap_setting` so an operator can see which
/// endpoint hit which cap. The permit moves into the socket task and frees the
/// slot when the task returns, so each class bounds its own live socket count
/// independently (see the `ws_*_semaphore` fields on [`AppState`]). Returns
/// `Option` rather than `Result<_, Response>` so the large `Response` does not
/// bloat the `Err` variant (clippy `result_large_err`).
fn acquire_ws_permit(
    semaphore: &Arc<tokio::sync::Semaphore>,
    peer_ip: std::net::IpAddr,
    route: &str,
    cap_setting: &str,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    match Arc::clone(semaphore).try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            dux_core::logger::warn(&format!(
                "[server] {route} upgrade refused for {peer_ip}: connection cap reached \
                 ({cap_setting})"
            ));
            None
        }
    }
}

/// Which PTY a nested socket streams: an agent session's main provider PTY (keyed
/// by session id) or a companion terminal's PTY (keyed by terminal id). Both
/// resolve through the same engine write/resize routing (`pty_for`), so the socket
/// loop treats them identically once subscribed; they differ only in how the
/// upgrade handler validates the path and how the initial subscription is taken.
enum PtyTarget {
    Agent(String),
    Terminal(String),
    /// An extra tab's provider PTY, keyed by tab id. Resolves through the same
    /// tab-keyed `providers` map as `Agent` (Main's tab id == session id), so the
    /// socket loop treats it identically once subscribed.
    Tab(String),
}

impl PtyTarget {
    /// The id used to route stdin writes and resizes (the session id for an agent,
    /// the terminal id for a companion terminal, the tab id for an extra tab). The
    /// engine's `pty_for` accepts any of these keyspaces.
    fn pty_id(&self) -> &str {
        match self {
            PtyTarget::Agent(id) | PtyTarget::Terminal(id) | PtyTarget::Tab(id) => id,
        }
    }
}

/// A resize control frame on a PTY socket: the Text frame
/// `{"rows":R,"cols":C}`, optionally `{"rows":R,"cols":C,"takeover":true}`,
/// distinct from the Binary stdin frames. Routed to `engine.resize_pty` for the
/// socket's own PTY ONLY when [`PtySizeOwners::claim_for_resize`] grants it: the
/// sender already owns sizing, the pty is unowned, or the frame explicitly asks
/// to take over. A non-owner's plain resize applies nothing and transfers
/// nothing, so two viewers of one PTY cannot thrash its size last-writer-wins
/// and an ordinary attach cannot steal the prompt.
///
/// `takeover` defaults to false, so a client too old to send it behaves exactly
/// as it always did in the one case where its claim was granted anyway (an
/// unowned pty). Its Take over button dies silently against a new server; the
/// mitigation is the run-identity hard reload, which replaces a stale page as
/// soon as the server run changes, so the window is one server run at most.
#[derive(serde::Deserialize)]
struct PtyResizeFrame {
    rows: u16,
    cols: u16,
    /// Set by a deliberate press of Take over, and by the one press-less
    /// re-claim the design keeps: a returning owner succeeding its own dead
    /// connection. The latter always carries `expected_owner`; the press never
    /// does.
    #[serde(default)]
    takeover: bool,
    /// The connection id this take-over believes currently owns the pty, sent
    /// ONLY by a returning owner succeeding the pane's previous, dead
    /// connection. A string for the same reason `input_owner` on the spine and
    /// `owner` on the handshake are: that is the shape the client holds ids in.
    ///
    /// Absent means "take from whoever holds it", which is what a PRESSED Take
    /// over sends. Present narrows the claim to one predecessor, so a frame
    /// delayed on a mobile radio cannot steal a pty that somebody else
    /// legitimately claimed in the gap. See
    /// [`PtySizeOwners::claim_for_resize`] for the decision itself.
    #[serde(default)]
    expected_owner: Option<String>,
}

/// A connection id no live connection can ever hold, used to spell "this frame
/// named an expected owner that could not be understood".
///
/// Connection ids come from a process-global counter that starts at zero and
/// increments once per socket open, so reaching `u64::MAX` would take more
/// socket opens than a machine can perform. Comparing against it therefore
/// always fails, which is exactly the verdict a malformed value deserves.
const UNMATCHABLE_CONN_ID: u64 = u64::MAX;

/// Read a resize frame's `expected_owner` into the form
/// [`PtySizeOwners::claim_for_resize`] takes.
///
/// Absent stays absent ("take from anyone"). A parseable id passes through. An
/// UNPARSEABLE value becomes [`UNMATCHABLE_CONN_ID`] rather than `None`, and
/// that distinction is the whole point of the function: treating garbage as "no
/// expectation" would promote a malformed frame to an unconditional take-over,
/// which is a silent steal, and the client that sent it is by definition
/// confused about who owns what.
fn parse_expected_owner(raw: Option<&str>) -> Option<u64> {
    raw.map(|value| value.parse::<u64>().unwrap_or(UNMATCHABLE_CONN_ID))
}

/// The ONE periodic control frame on a PTY socket: the Text frame
/// `{"beat":N,"viewed":B}`, distinct from the resize frame and the Binary stdin
/// frames. It carries two things that happen on the same cadence.
///
/// `viewed` is the older half: the frontend terminal sets it while it is the
/// input owner and its document is visible, so an agent the user is actively
/// watching keeps its attention flag down without requiring keystrokes. Routed
/// to `engine.note_viewed`, which self-gates on the pty id being a real agent
/// tab. A WATCHER sends the frame too, with `viewed` false, because the other
/// half is not about the owner: suppressing attention for everybody on a
/// watcher's behalf would be wrong.
///
/// `beat` is the new half, and it exists because the server's own 30s WebSocket
/// ping ([`WS_LIVENESS_PING_PERIOD`]) is send-only with no pong deadline. That
/// reaps a socket the OS has already given up on, but it cannot see the
/// half-open socket a Wi-Fi to cellular handoff leaves behind, where both ends
/// still believe they are connected. An application-level number the server
/// echoes back gives the browser a round trip it can time out on. Folded into
/// the viewed frame rather than added beside it because they run on the same
/// timer and a second periodic frame is a second thing to keep in step.
///
/// `viewed` carries `#[serde(default)]` so a client that sends only the beat is
/// read as a watcher rather than rejected.
#[derive(serde::Deserialize)]
// `deny_unknown_fields` is what tells this frame apart from a resize now that
// `beat` is optional: both of its fields have defaults, so without it a resize
// frame that failed its own parse would fall through and be read as an empty
// beat. The forward-compatibility cost is accepted and small: the two frames on
// this socket are known, and a browser running against a server it was not
// served by hard reloads.
#[serde(deny_unknown_fields)]
struct PtyBeatFrame {
    /// Absent on a page that predates the fold of the viewed ping into this one
    /// message, which sent a bare `{"viewed":true}`. Optional rather than
    /// required so such a frame still parses and its `viewed` half still counts:
    /// a required `beat` made the whole frame unparseable, silently dropping the
    /// attention signal. That window is short (a changed server run hard reloads
    /// the page) but it is not zero, because the run-identity check treats an
    /// unreachable endpoint as no evidence of a change.
    #[serde(default)]
    beat: Option<u64>,
    #[serde(default)]
    viewed: bool,
}

/// Upgrade handler for `GET /ws/sessions/:id/pty` — stream the agent session's main
/// provider PTY. Replicates the `/ws` protections (origin check, connection-cap
/// permit, frame-size limit) and path-validates `:id` against a known session
/// (404 otherwise, before the upgrade).
async fn ws_session_pty_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !same_origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin WebSocket upgrade rejected",
        )
            .into_response();
    }
    // Validate the session exists BEFORE the upgrade so a bad id is a clean HTTP
    // 404 rather than a socket that opens and immediately closes. Length-bound the
    // id first so a huge path can't drive an engine lookup.
    if !crate::rest_common::id_within_bound(&id)
        || state.engine.session_worktree(id.clone()).await.is_none()
    {
        return (StatusCode::NOT_FOUND, "unknown session").into_response();
    }
    let permit = match acquire_ws_permit(
        &state.ws_agent_semaphore,
        peer.ip(),
        "/ws/sessions/:id/pty",
        "max_websocket_agent_connections",
    ) {
        Some(permit) => permit,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "too many WebSocket connections; try again shortly",
            )
                .into_response();
        }
    };
    let engine = state.engine.clone();
    let console = state.console.clone();
    let pty_size_owners = Arc::clone(&state.pty_size_owners);
    let pty_grid_bus = Arc::clone(&state.pty_grid_bus);
    let bus = Arc::clone(&state.event_bus);
    let connections = Arc::clone(&state.connections);
    let live_limits = Arc::clone(&state.live_limits);
    let peer_ip = peer.ip();
    // Capture the claiming connection's User-Agent before the upgrade so the eventual
    // `pty.owner` handover can name this device to other viewers.
    let user_agent = captured_user_agent(&headers);
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_pty_socket(
                socket,
                engine,
                PtyTarget::Agent(id),
                console,
                peer_ip,
                permit,
                pty_size_owners,
                pty_grid_bus,
                bus,
                connections,
                user_agent,
                live_limits,
            )
        })
        .into_response()
}

/// Upgrade handler for `GET /ws/sessions/:id/terminals/:tid/pty` — stream a
/// companion terminal's PTY. Same protections as the agent socket, and
/// path-validates BOTH that `:id` is a known session AND that `:tid` belongs to it
/// (the legacy `SubscribeTerminal` looked terminals up by id alone; here the path
/// enforces session ownership). Either failing is a 404 before the upgrade.
async fn ws_terminal_pty_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path((id, tid)): Path<(String, String)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !same_origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin WebSocket upgrade rejected",
        )
            .into_response();
    }
    if !crate::rest_common::id_within_bound(&id) || !crate::rest_common::id_within_bound(&tid) {
        return (StatusCode::NOT_FOUND, "unknown terminal").into_response();
    }
    if state.engine.session_worktree(id.clone()).await.is_none() {
        return (StatusCode::NOT_FOUND, "unknown session").into_response();
    }
    // Route membership through the owner type's exhaustive `is_at_route`: an
    // unknown terminal, one owned by a different session, or a project terminal
    // is a 404 (never a cross-owner attach).
    match state.engine.terminal_owner_of(tid.clone()).await {
        Some(owner) if owner.is_at_route(dux_core::model::TerminalRoute::Session(&id)) => {}
        _ => return (StatusCode::NOT_FOUND, "unknown terminal").into_response(),
    }
    let permit = match acquire_ws_permit(
        &state.ws_terminal_semaphore,
        peer.ip(),
        "/ws/sessions/:id/terminals/:tid/pty",
        "max_websocket_terminal_connections",
    ) {
        Some(permit) => permit,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "too many WebSocket connections; try again shortly",
            )
                .into_response();
        }
    };
    let engine = state.engine.clone();
    let console = state.console.clone();
    let pty_size_owners = Arc::clone(&state.pty_size_owners);
    let pty_grid_bus = Arc::clone(&state.pty_grid_bus);
    let bus = Arc::clone(&state.event_bus);
    let connections = Arc::clone(&state.connections);
    let live_limits = Arc::clone(&state.live_limits);
    let peer_ip = peer.ip();
    let user_agent = captured_user_agent(&headers);
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_pty_socket(
                socket,
                engine,
                PtyTarget::Terminal(tid),
                console,
                peer_ip,
                permit,
                pty_size_owners,
                pty_grid_bus,
                bus,
                connections,
                user_agent,
                live_limits,
            )
        })
        .into_response()
}

/// Upgrade handler for `GET /ws/projects/:id/terminals/:tid/pty`: stream a
/// project terminal's PTY. Mirrors `ws_terminal_pty_upgrade` with the project
/// (not a session) as the path owner: same-origin check, id bounds, a
/// project-exists check, then per-variant ownership. The PTY plumbing downstream
/// of `PtyTarget::Terminal` is owner-blind, so the attach itself is identical.
async fn ws_project_terminal_pty_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path((id, tid)): Path<(String, String)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !same_origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin WebSocket upgrade rejected",
        )
            .into_response();
    }
    if !crate::rest_common::id_within_bound(&id) || !crate::rest_common::id_within_bound(&tid) {
        return (StatusCode::NOT_FOUND, "unknown terminal").into_response();
    }
    if state.engine.project_path(id.clone()).await.is_none() {
        return (StatusCode::NOT_FOUND, "unknown project").into_response();
    }
    // Route membership through the same exhaustive `is_at_route`: an unknown
    // terminal, a session-owned terminal, or one owned by a different project is
    // a 404 (never a cross-owner attach).
    match state.engine.terminal_owner_of(tid.clone()).await {
        Some(owner) if owner.is_at_route(dux_core::model::TerminalRoute::Project(&id)) => {}
        _ => return (StatusCode::NOT_FOUND, "unknown terminal").into_response(),
    }
    let permit = match acquire_ws_permit(
        &state.ws_terminal_semaphore,
        peer.ip(),
        "/ws/projects/:id/terminals/:tid/pty",
        "max_websocket_terminal_connections",
    ) {
        Some(permit) => permit,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "too many WebSocket connections; try again shortly",
            )
                .into_response();
        }
    };
    let engine = state.engine.clone();
    let console = state.console.clone();
    let pty_size_owners = Arc::clone(&state.pty_size_owners);
    let pty_grid_bus = Arc::clone(&state.pty_grid_bus);
    let bus = Arc::clone(&state.event_bus);
    let connections = Arc::clone(&state.connections);
    let live_limits = Arc::clone(&state.live_limits);
    let peer_ip = peer.ip();
    let user_agent = captured_user_agent(&headers);
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_pty_socket(
                socket,
                engine,
                PtyTarget::Terminal(tid),
                console,
                peer_ip,
                permit,
                pty_size_owners,
                pty_grid_bus,
                bus,
                connections,
                user_agent,
                live_limits,
            )
        })
        .into_response()
}

/// Upgrade handler for `GET /ws/terminals/:tid/pty`: stream a STANDALONE
/// terminal's PTY. Un-nested, because a standalone terminal has no owner to nest
/// under, so there is no owner-exists check to run first; the ownership check
/// that remains is the important half, and it goes the other way: the exhaustive
/// `is_at_route` refuses an owned terminal here, so this address cannot be used
/// to attach to a session's or a project's terminal without its path owner.
async fn ws_standalone_terminal_pty_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(tid): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !same_origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin WebSocket upgrade rejected",
        )
            .into_response();
    }
    if !crate::rest_common::id_within_bound(&tid) {
        return (StatusCode::NOT_FOUND, "unknown terminal").into_response();
    }
    match state.engine.terminal_owner_of(tid.clone()).await {
        Some(owner) if owner.is_at_route(dux_core::model::TerminalRoute::Standalone) => {}
        _ => return (StatusCode::NOT_FOUND, "unknown terminal").into_response(),
    }
    let permit = match acquire_ws_permit(
        &state.ws_terminal_semaphore,
        peer.ip(),
        "/ws/terminals/:tid/pty",
        "max_websocket_terminal_connections",
    ) {
        Some(permit) => permit,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "too many WebSocket connections; try again shortly",
            )
                .into_response();
        }
    };
    let engine = state.engine.clone();
    let console = state.console.clone();
    let pty_size_owners = Arc::clone(&state.pty_size_owners);
    let pty_grid_bus = Arc::clone(&state.pty_grid_bus);
    let bus = Arc::clone(&state.event_bus);
    let connections = Arc::clone(&state.connections);
    let live_limits = Arc::clone(&state.live_limits);
    let peer_ip = peer.ip();
    let user_agent = captured_user_agent(&headers);
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_pty_socket(
                socket,
                engine,
                PtyTarget::Terminal(tid),
                console,
                peer_ip,
                permit,
                pty_size_owners,
                pty_grid_bus,
                bus,
                connections,
                user_agent,
                live_limits,
            )
        })
        .into_response()
}

/// RAII guard for the per-agent live-tab-socket sub-quota
/// (`[server] max_websocket_tabs_per_agent`). `acquire` increments the owning
/// session's count when it is below the cap (returning `None` to refuse at/over
/// the cap, and — because `0 >= 0` — when the cap is `0`, which blocks all tab
/// sockets); `Drop` decrements it, so every early return and socket close releases
/// the slot. The count map is shared via `AppState.tab_ws_counts`.
struct TabWsGuard {
    counts: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    session_id: String,
}

impl TabWsGuard {
    fn acquire(
        counts: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
        session_id: String,
        max_per_agent: u32,
    ) -> Option<Self> {
        {
            let mut map = counts.lock().unwrap_or_else(|e| e.into_inner());
            let n = map.entry(session_id.clone()).or_insert(0);
            if *n as u32 >= max_per_agent {
                // At/over the cap (or `max_per_agent == 0` blocks all). Don't leave
                // a freshly-created zero entry lingering in the map.
                if *n == 0 {
                    map.remove(&session_id);
                }
                return None;
            }
            *n += 1;
        }
        Some(Self { counts, session_id })
    }
}

impl Drop for TabWsGuard {
    fn drop(&mut self) {
        let mut map = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = map.get_mut(&self.session_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(&self.session_id);
            }
        }
    }
}

/// Upgrade handler for `GET /ws/sessions/:id/tabs/:tab/pty` — stream a Support
/// tab's provider PTY. Support-only (the session-slot tab uses `/ws/sessions/:id/pty`).
/// Validates origin, id bounds, session existence, and extra-tab ownership
/// (`:tab` belongs to `:id`), then takes a permit from the DEDICATED tab-socket
/// pool (`ws_tab_semaphore`, sized by `max_websocket_tab_connections`) — separate
/// from the agent-PTY pool, so tab sockets can never 503 the session-slot tab streams.
/// Each failing branch is a 404/503 BEFORE the upgrade.
async fn ws_tab_pty_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !same_origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin WebSocket upgrade rejected",
        )
            .into_response();
    }
    if !crate::rest_common::id_within_bound(&id) || !crate::rest_common::id_within_bound(&tab) {
        return (StatusCode::NOT_FOUND, "unknown tab").into_response();
    }
    if state.engine.session_worktree(id.clone()).await.is_none() {
        return (StatusCode::NOT_FOUND, "unknown session").into_response();
    }
    // Support-only ownership: a session-slot tab has no `agent_tabs` row, so `tab_session`
    // returns `None` and this 404s (Main streams over `/ws/sessions/:id/pty`).
    match state.engine.tab_session(tab.clone()).await {
        Some(owner) if owner == id => {}
        _ => return (StatusCode::NOT_FOUND, "unknown tab").into_response(),
    }
    // Per-agent fairness sub-quota: refuse a new tab socket for a session already
    // at `max_ws_tabs_per_agent` BEFORE taking a shared tab-pool permit, so one
    // agent's tabs can't monopolize that pool. The guard decrements on drop, so a
    // failed permit acquisition below (early return) also releases the slot.
    let tab_guard = match TabWsGuard::acquire(
        Arc::clone(&state.tab_ws_counts),
        id.clone(),
        state.max_ws_tabs_per_agent,
    ) {
        Some(guard) => guard,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "too many tab connections for this agent; try again shortly",
            )
                .into_response();
        }
    };
    let permit = match acquire_ws_permit(
        &state.ws_tab_semaphore,
        peer.ip(),
        "/ws/sessions/:id/tabs/:tab/pty",
        "max_websocket_tab_connections",
    ) {
        Some(permit) => permit,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "too many WebSocket connections; try again shortly",
            )
                .into_response();
        }
    };
    let engine = state.engine.clone();
    let console = state.console.clone();
    let pty_size_owners = Arc::clone(&state.pty_size_owners);
    let pty_grid_bus = Arc::clone(&state.pty_grid_bus);
    let bus = Arc::clone(&state.event_bus);
    let connections = Arc::clone(&state.connections);
    let live_limits = Arc::clone(&state.live_limits);
    let peer_ip = peer.ip();
    let user_agent = captured_user_agent(&headers);
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| async move {
            // Hold the per-agent sub-quota guard for the socket's lifetime; it
            // decrements the agent's tab-socket count when this future is dropped.
            let _tab_guard = tab_guard;
            handle_pty_socket(
                socket,
                engine,
                PtyTarget::Tab(tab),
                console,
                peer_ip,
                permit,
                pty_size_owners,
                pty_grid_bus,
                bus,
                connections,
                user_agent,
                live_limits,
            )
            .await
        })
        .into_response()
}

/// Drive one nested per-PTY socket. On open, subscribe to the target PTY and replay
/// the buffered scrollback/repaint (sized to `agent_scrollback_lines` inside the
/// `PtyClient`). Then:
/// server→client is Binary frames of raw PTY bytes; a client→server Binary frame is
/// PTY stdin; a client→server Text frame `{"rows":R,"cols":C}` is a resize applied
/// only while this connection owns sizing (see [`PtySizeOwners`]). Close (or any
/// stream end) detaches by dropping the subscription/forwarder and releasing
/// sizing ownership.
#[allow(clippy::too_many_arguments)]
async fn handle_pty_socket(
    socket: WebSocket,
    engine: EngineHandle,
    target: PtyTarget,
    console: Console,
    peer_ip: std::net::IpAddr,
    // Held for the socket's lifetime purely for its Drop (frees a connection-cap
    // slot when this returns). Never read.
    _permit: tokio::sync::OwnedSemaphorePermit,
    pty_size_owners: Arc<PtySizeOwners>,
    // The grid-change bus. This socket both PUBLISHES to it (when its own
    // resize is granted) and LISTENS on it (for every other connection's), so
    // a viewer of this PTY is told the authoritative grid it is not rendering
    // at.
    pty_grid_bus: Arc<crate::pty_sizes::PtyGridBus>,
    bus: Arc<EventBus>,
    connections: Arc<crate::rest_common::ConnectionRegistry>,
    // The claiming connection's raw `User-Agent`, captured at the upgrade before the
    // socket split. This connection IS the claimer at both `pty.owner` emit sites, so
    // the value is a plain local (no per-conn map needed); it rides the handover so a
    // client on another device can name this one ("Chrome on macOS").
    user_agent: Option<String>,
    // The reloadable `[server]` scalars. Exactly one is read here, at open:
    // `pty_send_timeout_seconds`, which bounds the two opening sends. Read per
    // socket rather than frozen at bind, so a config reload retimes the next
    // connection.
    live_limits: Arc<crate::engine_actor::LiveServerLimits>,
) {
    console.client_connected(peer_ip);
    // Register this PTY socket as a live connection (its class depends on which PTY
    // family it streams), so the liveness reaper and per-class counts see it. The
    // id is a fresh server UUID (PTY sockets carry no client-facing id of their
    // own); the guard deregisters on every exit path.
    let registry_id = uuid::Uuid::new_v4().to_string();
    let conn_class = match &target {
        // extra tabs run provider CLIs, so they count as agent-PTY connections.
        PtyTarget::Agent(_) | PtyTarget::Tab(_) => crate::rest_common::ConnClass::AgentPty,
        PtyTarget::Terminal(_) => crate::rest_common::ConnClass::TerminalPty,
    };
    connections.insert(registry_id.clone(), conn_class);
    let _conn_guard = ConnectionGuard {
        id: registry_id,
        registry: Arc::clone(&connections),
    };
    // The console's live-client count, decremented on every exit path including
    // an unwind. Declared immediately after the increment above so the pair is
    // read as one thing.
    let _client_count_guard = ClientCountGuard {
        console: console.clone(),
        peer_ip,
    };
    let (sink, mut stream) = socket.split();
    let sink: SharedSink = Arc::new(tokio::sync::Mutex::new(sink));

    // Subscribe to the target PTY. An agent subscribe also launches/resumes the
    // provider if it isn't running yet (the same flow the legacy Subscribe uses);
    // a terminal subscribe attaches to an already-created companion terminal.
    let subscription = match &target {
        // A tab subscribe resolves through the same tab-keyed `subscribe_pty`; for a
        // dormant extra tab this launches it via `launch_agent`, which resolves
        // resume vs. fresh dynamically per `tab_resume_decision` (not always fresh).
        PtyTarget::Agent(id) | PtyTarget::Tab(id) => engine.subscribe_pty(id.clone()).await,
        PtyTarget::Terminal(id) => engine.subscribe_terminal(id.clone()).await,
    };
    // Bind the guard for the socket's full lifetime. Dropping it when this
    // function returns removes the subscriber immediately on disconnect.
    let (_viewer_guard, repaint, rx) = match subscription {
        Ok(sub) => sub,
        Err(e) => {
            // Subscribe failed after the upgrade (e.g. the agent failed to launch,
            // or the terminal vanished in the gap). Close with the provider-gone
            // code so the client stops instead of reconnecting and re-launching
            // the doomed provider forever.
            dux_core::logger::warn(&format!(
                "PTY socket subscribe failed for {peer_ip} (pty {:?}): {e}",
                target.pty_id()
            ));
            {
                let mut guard = sink.lock().await;
                let _ = guard.send(provider_gone_close()).await;
            }
            // The console's count is decremented by `_client_count_guard` above,
            // on this return like any other; saying so again here would count
            // this socket out twice.
            return;
        }
    };
    // Allocate this connection's id, but do NOT claim ownership: attaching is not
    // taking over, and now the server means it. A connection becomes the owner
    // only by resizing an UNOWNED pty or by sending a resize explicitly flagged
    // as a take-over (see the resize arm below), so no attach of any kind can
    // steal the device that is actually being typed on. Ownership is released on
    // disconnect, through the guard declared immediately below.
    let conn_id = pty_size_owners.next_conn_id();
    // The release, on EVERY exit path. A socket that gave up on its opening
    // sends has to let go of the pty too (it may have claimed it at the
    // handshake), and so does one a panic unwinds through: leaving it held wedges
    // the pty behind a client that can never see it. `PtyOwnershipGuard` also
    // broadcasts the owner-cleared `pty.owner`, without which every other
    // device's card keeps naming a browser tab that closed.
    let _ownership_guard = PtyOwnershipGuard {
        pty_id: target.pty_id().to_string(),
        conn_id,
        owners: Arc::clone(&pty_size_owners),
        bus: Arc::clone(&bus),
    };
    // Who is driving right now, read once for the handshake frame. This is what
    // stops a foregrounded arrival from wedging itself as a phantom owner: with
    // a plain claim now refused SILENTLY, the client's optimistic "I am
    // foregrounded so I must be the owner" guess would never be corrected, and it
    // would render typing surfaces over a pty whose every keystroke the server
    // drops. The client seeds its verdict from this and keeps the foreground
    // guess only for deciding whether to claim an UNOWNED pty.
    //
    // The ownership epoch is read in the SAME lock acquisition as the owner and
    // travels on the frame: the handshake rides the PTY socket while `pty.owner`
    // broadcasts ride the events socket, two TCP connections with no ordering
    // between them. Stamping the snapshot lets a client that has already applied
    // a strictly newer `pty.owner` recognize this frame as stale and keep the
    // newer verdict, instead of re-seeding itself as a phantom owner from an
    // outdated `owner: null` that nothing would ever correct.
    // The owner's device label rides the same snapshot: it is recorded with the
    // owner id at claim time, so this one lock acquisition answers who drives,
    // since when, and from what device. A mere attach broadcasts no `pty.owner`,
    // so the handshake is the only frame that can name the driving device to a
    // watcher that simply opened the pane.
    let owner_snapshot = pty_size_owners.current_owner(target.pty_id());
    // The seq of the last resize that actually REACHED the child, read BEFORE the
    // actor-queued grid read below, which makes it a valid LOWER bound for the
    // grid the handshake carries: the grid read is enqueued behind every resize
    // already accepted, and the actor drains in order. Seeded into this socket's
    // forwarding filter and handed to the client on the handshake, so a stale
    // broadcast still in flight when the handshake was sent can never regress
    // the grid after it. Reading it early only errs towards forwarding a
    // redundant same-geometry change, which the client de-duplicates.
    //
    // The APPLIED mark and not the STAMPED one, and that is a distinction with
    // teeth. The terminal UI stamps its claim and applies in a second step, and a
    // browser's resize is stamped and then queued, so a handshake that lands
    // inside either window WOULD seed its filter at N while the grid it carries
    // is still the pre-N geometry; the apply's own broadcast then arrives stamped
    // N, is dropped as "not newer", and nothing ever re-announces it: that viewer
    // sits on a stale grid for the life of the socket.
    let handshake_grid_seq = pty_size_owners.applied_grid_seq(target.pty_id());
    // Hand the client this PTY socket's connection id as the first frame (a Text
    // frame, distinct from the Binary PTY-byte frames), mirroring how `/ws/events`
    // opens with a `connected` frame. The client records it and compares it against
    // the `owner` field of every `pty.owner` event: an equal id means this client
    // is the (new) owner, a different id means another device took over. This
    // comparison is definitive: two devices claiming at once cannot both conclude
    // they lost. A fresh id is allocated per
    // socket open, so a reconnect re-issues one.
    // Stamp this (re)open's scrollback replay with a process-monotonic generation
    // id and hand it to the client on the `connected` handshake, immediately before
    // the replay Binary frame it labels. The client records the last generation it
    // applied and drops any replay whose generation it has already applied, so a
    // duplicate replay or a late blob from a torn-down forwarder can never stack a
    // second copy of the scrollback on top of the buffer (the mobile
    // duplicated-text bug). A fresh generation per open makes every legitimate
    // reconnect strictly newer, so the guard only ever fires on the anomaly.
    let replay_generation = next_replay_generation();
    // Subscribe to the grid-change bus BEFORE reading the grid for the
    // handshake, and before the loop starts. A broadcast receiver does not
    // replay sends that happened before it existed, so subscribing after the
    // read would leave a hole: a resize landing between the read and the
    // subscribe would be reported to nobody on this socket and its viewer would
    // sit on a stale grid until the next change.
    let mut grid_changes = pty_grid_bus.subscribe();
    // The grid the child is actually drawing for, read once for the handshake.
    // A viewer compares it against its own xterm's grid: one PTY has one
    // authoritative grid (the owner's), and every other attached browser is
    // rendering the same byte stream into a differently sized terminal without,
    // until this frame, any way to know it.
    let grid = engine.pty_grid_size(target.pty_id().to_string()).await;
    // Read ONCE per socket, before the two sends it bounds, so both halves of an
    // attach share one deadline even if a reload lands between them.
    let opening_send_deadline = pty_opening_send_timeout(&live_limits);
    'attached: {
        // A client that never receives its handshake or its replay sees a
        // permanently blank terminal pane on a connection that looks alive from both
        // ends, so both sends are BOUNDED and CHECKED. Either one giving up leaves
        // this block, which drops straight into the ordinary teardown below: the pty
        // ownership is released and the connection permit is freed, exactly as a
        // clean disconnect would. Proceeding into the select loop instead would park
        // a socket nobody can ever see anything on.
        if with_send_deadline(
            opening_send_deadline,
            send_pty_connected(
                &sink,
                conn_id,
                replay_generation,
                owner_snapshot,
                grid,
                handshake_grid_seq,
            ),
        )
        .await
        .is_err()
        {
            dux_core::logger::info(&crate::pty_log::describe_connection_reaped(
                conn_id,
                crate::pty_log::FailedSend::ConnectedHandshake,
            ));
            break 'attached;
        }
        // Replay the buffered scrollback/repaint before streaming live bytes.
        let replay_bytes = repaint.len();
        if with_send_deadline(opening_send_deadline, send_binary(&sink, repaint))
            .await
            .is_err()
        {
            dux_core::logger::info(&crate::pty_log::describe_connection_reaped(
                conn_id,
                crate::pty_log::FailedSend::ScrollbackReplay,
            ));
            break 'attached;
        }
        dux_core::logger::debug(&crate::pty_log::describe_replay_sent(
            target.pty_id(),
            replay_generation,
            replay_bytes,
        ));
        let mut pty_forwarder = spawn_pty_forwarder(Arc::clone(&sink), rx, engine.shutdown_flag());

        // Liveness ping (every connection). Consume the immediate first tick so the
        // first real ping waits a full period.
        let mut ping = tokio::time::interval(WS_LIVENESS_PING_PERIOD);
        ping.tick().await;

        // The newest grid seq this socket has forwarded, seeded from the
        // handshake's own read. Grid publishes happen after the owners lock
        // releases, so two sockets' announcements of two ordered applies can reach
        // the bus inverted; anything at or below this mark is older geometry than
        // the client already knows and is dropped here. The client keeps the same
        // filter itself (seeded from the handshake's `grid_seq`), which is the
        // guard that must exist; this one just saves the wire trip.
        let mut last_grid_seq = handshake_grid_seq;

        loop {
            let msg = tokio::select! {
                // Liveness ping: a failed send reaps a dead/half-open peer.
                _ = ping.tick() => {
                    if send_ping(&sink).await.is_err() {
                        dux_core::logger::info(&crate::pty_log::describe_connection_reaped(
                            conn_id,
                            crate::pty_log::FailedSend::LivenessPing,
                        ));
                        break;
                    }
                    continue;
                }
                // Somebody resized THIS pty (possibly this very connection). Push
                // the new grid down as a `size` event frame so a viewer can tell
                // that it is rendering the child's output at a geometry the child
                // is not drawing for. One arm in the loop this socket already runs,
                // rather than a registry holding other tasks' sinks; see
                // `pty_sizes.rs` for why.
                change = grid_changes.recv() => {
                    match change {
                        Ok(change) => {
                            if change.pty_id != target.pty_id() {
                                continue;
                            }
                            // A stale announcement (reordered after the newer one
                            // it lost to, or already covered by this socket's own
                            // handshake read): drop it rather than letting the
                            // older geometry become the client's last word.
                            if change.seq <= last_grid_seq {
                                continue;
                            }
                            last_grid_seq = change.seq;
                            let text = pty_size_frame_text(change.rows, change.cols, change.seq);
                            if !text.is_empty() && send_text(&sink, text).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        // Lagged: this socket fell behind the bus. Nothing to
                        // recover, and deliberately nothing sent: the NEXT change
                        // carries the current geometry, and a reconnect's handshake
                        // re-reads it from scratch. Keep listening.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        // Unreachable: this handler holds an `Arc` of the bus for
                        // its whole lifetime, so the sender cannot have been
                        // dropped while this receiver lives. Stated as a break
                        // rather than a `continue`, which would spin.
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                // The forwarder task ends when the PTY is torn down server-side
                // (close_tab/DetachAgent/crash) even while the client stays
                // connected. Without this arm the socket + its connection-cap
                // permit/guard linger until the client itself disconnects, which
                // can pin the small per-agent WS sub-quota. Proactively tell the
                // client to close and tear down our own loop the same way a
                // client-initiated Close would.
                _ = &mut pty_forwarder => {
                    // The PTY was torn down server-side. If we are shutting down the
                    // client should reconnect when the server returns (plain close);
                    // otherwise the provider crashed/exited or its tab/agent closed, so
                    // send the provider-gone code to stop the client relaunching it.
                    let shutting_down = engine
                        .shutdown_flag()
                        .load(std::sync::atomic::Ordering::SeqCst);
                    let mut guard = sink.lock().await;
                    let _ = guard.send(forwarder_end_close(shutting_down)).await;
                    break;
                }
                next = stream.next() => match next {
                    Some(Ok(msg)) => msg,
                    _ => break,
                },
            };
            match msg {
                // Binary frame = raw PTY stdin for THIS socket's PTY. The write gate is
                // resolved ATOMICALLY by `may_write` (holding the owners lock across the
                // decision) so no concurrent claim can slip between the check and the
                // write. A non-owner's stdin is dropped (with a diagnostic log) so a
                // read-only secondary viewer can never disrupt the active device's
                // typing; an UNOWNED PTY's first writer is allowed AND becomes the owner,
                // emitting one `pty.owner` so other clients update (the uncontested
                // out-of-band case that arrives before any size frame).
                Message::Binary(bytes) => {
                    let pty_id = target.pty_id();
                    let claim = pty_size_owners.may_write(pty_id, conn_id, user_agent.as_deref());
                    if claim.allowed {
                        // `epoch` is `Some` exactly when this write newly claimed an
                        // unowned PTY, so emit one handover stamped with that epoch.
                        if let Some(epoch) = claim.epoch {
                            dux_core::logger::info(&crate::pty_log::describe_claim_granted(
                                pty_id,
                                conn_id,
                                user_agent.as_deref(),
                                false,
                                None,
                            ));
                            bus.emit(pty_owner_event(
                                pty_id,
                                conn_id,
                                epoch,
                                user_agent.as_deref(),
                            ));
                        }
                        engine.write_pty(pty_id.to_string(), bytes.to_vec());
                    } else {
                        dux_core::logger::debug(&format!(
                            "PTY stdin from non-owner conn {conn_id} dropped for pty {pty_id} \
                             (another connection currently owns input)"
                        ));
                    }
                }
                // Text frame = a resize control message. The claim decision and the
                // resize itself are resolved ATOMICALLY by `claim_for_resize`: an
                // unowned pty is claimed, the current owner resizes freely, an
                // explicit `takeover` transfers ownership, and any other non-owner's
                // resize is refused whole (nothing applied, nothing broadcast) so
                // attaching cannot steal the device being typed on. On a real
                // handover we broadcast a `pty.owner` (carrying this connection's id)
                // so other clients viewing this PTY flip to the read-only take-over
                // placeholder.
                Message::Text(text) => {
                    if let Ok(frame) = serde_json::from_str::<PtyResizeFrame>(text.as_str()) {
                        let pty_id = target.pty_id();
                        let expected_owner = parse_expected_owner(frame.expected_owner.as_deref());
                        let outcome = pty_size_owners.claim_for_resize(
                            pty_id,
                            conn_id,
                            frame.takeover,
                            expected_owner,
                            user_agent.as_deref(),
                            |seq| {
                                engine.resize_pty(pty_id.to_string(), frame.rows, frame.cols, seq);
                            },
                        );
                        if let Some(epoch) = outcome.epoch {
                            dux_core::logger::info(&crate::pty_log::describe_claim_granted(
                                pty_id,
                                conn_id,
                                user_agent.as_deref(),
                                frame.takeover,
                                expected_owner,
                            ));
                            bus.emit(pty_owner_event(
                                pty_id,
                                conn_id,
                                epoch,
                                user_agent.as_deref(),
                            ));
                        }
                        if let Some(seq) = outcome.seq {
                            // The grid really moved, so tell every socket attached
                            // to this pty. Published here, after the owners lock is
                            // released, exactly like the `pty.owner` broadcast
                            // above it, and keyed on the seq (present exactly when
                            // the resize applied) so a refused resize (which
                            // changed nothing) announces nothing. The announced
                            // size is the one the closure enqueued, which is what
                            // the child will be drawing for; the seq, stamped in
                            // the same critical section, lets receivers drop this
                            // announcement if a newer one overtook it on the way
                            // out. One accepted anomaly: the closure's `try_send`
                            // can drop the resize under a full engine queue while
                            // this publish still announces it, and that is fine
                            // because the viewer's healing handshake re-reads the
                            // real grid, so the announcement self-corrects.
                            pty_grid_bus.publish(pty_id, frame.rows, frame.cols, seq);
                        } else {
                            // Logged at INFO rather than debug: "my keystrokes do
                            // nothing" is the report this line answers, and a debug
                            // line nobody has switched on answers nothing. It names
                            // the device that IS driving and which of the two
                            // refusals this was, because a plain non-owner resize
                            // and a superseded ghost succession look identical from
                            // the browser and mean very different things.
                            let reason = if frame.takeover {
                                crate::pty_log::ResizeRefusal::ExpectedOwnerMismatch
                            } else {
                                crate::pty_log::ResizeRefusal::NonOwnerPlainResize
                            };
                            let (current_owner, _, _) = pty_size_owners.current_owner(pty_id);
                            dux_core::logger::info(&crate::pty_log::describe_claim_refused(
                                pty_id,
                                conn_id,
                                current_owner,
                                reason,
                            ));
                        }
                    } else if let Ok(frame) = serde_json::from_str::<PtyBeatFrame>(text.as_str()) {
                        // The viewed half never claims sizing ownership; it only
                        // stamps the engine's engagement window for this pty's tab.
                        // The engine ignores it for a non-tab (companion) or unknown
                        // id. A watcher's frame sets it false and must not suppress
                        // attention for everybody.
                        if frame.viewed {
                            engine.note_viewed(target.pty_id().to_string());
                        }
                        // The beat half is answered unconditionally, watcher
                        // included: every attached page needs a round trip it can
                        // time out on, not just the one that is driving. A failed
                        // send here is left to the ping reaper rather than breaking
                        // the loop, because a single dropped answer is exactly what
                        // the browser's own deadline is for.
                        //
                        // BOUNDED for the same reason the opening sends are: this
                        // holds the shared sink lock while it waits, so an
                        // unbounded one against a wedged peer wedges every other
                        // write on the socket behind it, which is precisely the
                        // argument in `pty_opening_send_timeout`. It does NOT
                        // share that deadline, though: that one is a throughput
                        // allowance for a whole scrollback replay, and this is
                        // twenty-five bytes. See `PTY_BEAT_ECHO_TIMEOUT`.
                        //
                        // A frame with no `beat` is a page that predates the fold
                        // of the viewed ping into this message. Its `viewed` half
                        // is still honored above; there is simply nothing to echo.
                        if let Some(n) = frame.beat {
                            let _ = with_send_deadline(
                                PTY_BEAT_ECHO_TIMEOUT,
                                send_text(&sink, pty_beat_frame_text(n)),
                            )
                            .await;
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }

        // Detach: stop the forwarder so it doesn't linger on the subscription.
        // Inside the block, because a socket that gave up before the forwarder
        // was spawned has none to stop.
        pty_forwarder.abort();
    }

    // Ownership release and the console's client count are the two GUARDS
    // declared at the top of this function, so they run here by falling out of
    // scope rather than by being called. See `PtyOwnershipGuard` and
    // `ClientCountGuard`: they used to be two plain statements at the end, which
    // a panic unwinding through the handler skipped, leaving a phantom owner on
    // the pty for the life of the process and a client count that only ever went
    // up. `_conn_guard` above them has been a Drop guard for exactly this reason
    // all along.
}

/// A `pty.owner` signal: the connection that owns a PTY's sizing+input changed (a
/// new device pressed Take over, a client sized an UNOWNED pty, or an uncontested
/// first writer claimed one). A plain resize against a pty somebody else owns
/// emits nothing at all, because it changes nothing. `id` is the pty id (the session id for an agent PTY,
/// the terminal id for a companion); `owner` is the claiming connection's id (the
/// `PtySizeOwners` conn id). It carries no `rev`. Delivered on the coarse
/// `sessions` topic (every client holds it), so any other client currently viewing
/// that PTY compares `owner` against its own PTY-socket connection id: an equal id
/// means this client is the owner; a different id flips it to the read-only
/// take-over placeholder. The explicit id is definitive: without it, two devices
/// claiming at once could both end up showing the placeholder while the server
/// held a real owner. `epoch` is the
/// monotonic ownership epoch assigned under the owners lock (see [`OwnersState`]);
/// it orders concurrent handovers so a client can ignore an out-of-order broadcast
/// and keep only the latest claim, since this event is emitted AFTER the lock
/// releases and the runtime may reorder two near-simultaneous broadcasts.
/// `device` is the claimer's raw `User-Agent` (captured at its PTY upgrade), which
/// the client parses into a human label ("Chrome on macOS") for the take-over
/// placeholder; it is `None` when the claimer sent no `User-Agent`.
pub(crate) fn pty_owner_event(
    pty_id: &str,
    owner_conn_id: u64,
    epoch: u64,
    device: Option<&str>,
) -> Event {
    Event::Resource {
        event: "pty.owner".to_string(),
        id: Some(pty_id.to_string()),
        rev: None,
        owner: Some(owner_conn_id.to_string()),
        epoch: Some(epoch),
        device: device.map(str::to_owned),
    }
}

/// The OWNER-CLEARED `pty.owner`: same event, no `owner` field, emitted when the
/// driving connection disconnects and its ownership is released. Every client
/// reads a missing owner as "not me" (see `isOwnerAfterHandover`), so all of them
/// converge on "nobody is driving"; a mounted, foregrounded viewer then claims
/// the freed pty and a backgrounded one switches its card's copy.
///
/// It exists because ownership stopped following focus. Before that, a departed
/// owner was corrected by the next device that happened to attach or alt-tab,
/// which was also the silent steal; with the steal gone, this is the ONLY signal
/// that the other device has left, and without it the take-over card becomes a
/// permanent lie about a browser tab that closed. It carries the epoch assigned
/// under the owners lock by the release itself, so the client's epoch ordering
/// places it correctly against the claim it retires, and no `device` (there is
/// no claimer to name).
pub(crate) fn pty_owner_cleared_event(pty_id: &str, epoch: u64) -> Event {
    Event::Resource {
        event: "pty.owner".to_string(),
        id: Some(pty_id.to_string()),
        rev: None,
        owner: None,
        epoch: Some(epoch),
        device: None,
    }
}

/// The single `config.changed` signal emitted whenever the engine reloads config.
/// No `id`/`rev` — it is a plain "refetch `/api/v1/bootstrap`" signal delivered on
/// the coarse `config` topic.
pub(crate) fn config_changed_event() -> Event {
    Event::Resource {
        event: "config.changed".to_string(),
        id: None,
        rev: None,
        owner: None,
        epoch: None,
        device: None,
    }
}

/// Bridge engine config reloads onto the event bus as `config.changed`. The engine
/// actor fires `()` on its reload broadcast after each successful reload; this task
/// re-emits a `config.changed` event so subscribed clients refetch bootstrap. A
/// `Lagged` recovery still only needs to say "config changed" once (the signal is
/// value-less and idempotent), so missed reloads coalesce into a single emit.
/// Exits when the engine — and thus the reload broadcast — is gone. Returns the
/// task handle (used by tests; the production caller fire-and-forgets it).
fn spawn_config_changed_forwarder(
    mut reload_rx: tokio::sync::broadcast::Receiver<()>,
    bus: Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match reload_rx.recv().await {
                Ok(()) => bus.emit(config_changed_event()),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    bus.emit(config_changed_event())
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// A coarse `projects.changed` signal: no `id`/`rev`, just "refetch the projects
/// read" (`/api/v1/projects` or `/api/v1/workspace`), delivered on the `projects`
/// topic.
///
/// The workspace document itself is PUSHED alongside this signal (see
/// [`workspace_frame_text`]), so a client that understands the pushed frame stops
/// refetching on this one. It keeps firing regardless: it is what a page from an
/// older build still needs, it costs nothing, and removing it is a separate
/// cleanup with its own compatibility argument to make.
fn projects_changed_event() -> Event {
    Event::Resource {
        event: "projects.changed".to_string(),
        id: None,
        rev: None,
        owner: None,
        epoch: None,
        device: None,
    }
}

/// A coarse `sessions.changed` signal: no `id`/`rev`, just "refetch the sessions
/// read" (`/api/v1/sessions` or `/api/v1/workspace`), delivered on the `sessions`
/// topic. Covers session lifecycle/status, the `working` flag, and the terminal
/// list (they all live in the sessions/sidebar projection). Like
/// `projects.changed`, it now travels alongside the pushed document rather than
/// being the only way a client learns of the change.
fn sessions_changed_event() -> Event {
    Event::Resource {
        event: "sessions.changed".to_string(),
        id: None,
        rev: None,
        owner: None,
        epoch: None,
        device: None,
    }
}

/// The pushed workspace-document frame:
/// `{"event":"workspace","rev":N,"workspace":{…}}`.
///
/// Built by splicing the cached serialization into a string rather than by
/// serializing a struct. The document is already JSON in the engine's cache and
/// every subscribed connection is sent the same bytes; re-serializing it per
/// connection is exactly the cost this frame exists to remove. The revision
/// appears twice on purpose: once at the top level, where a client reads it
/// without touching the body, and once inside the document, where a client that
/// FETCHED the same bytes over REST finds it.
fn workspace_frame_text(doc: &WorkspaceDoc) -> String {
    format!(
        r#"{{"event":"workspace","rev":{},"workspace":{}}}"#,
        doc.rev, doc.json
    )
}

/// Whether this connection asked for the workspace document. It rides the two
/// coarse topics that carry the `projects.changed` / `sessions.changed` pings,
/// because it is the document those pings tell the client to refetch. Either
/// topic is enough: the document is not split in half.
fn holds_workspace_topic(subscribed: &std::collections::HashSet<String>) -> bool {
    subscribed.contains("sessions") || subscribed.contains("projects")
}

/// Bridge engine spine changes onto the event bus as coarse `projects.changed` /
/// `sessions.changed` events. The engine loop fires a [`SpineChange`] per changed
/// side; this task re-emits the matching event so subscribed clients refetch
/// `/api/v1/workspace` (a page too old to read the pushed document). On `Lagged`
/// it re-emits BOTH coarse signals once (the signals
/// are value-less and idempotent, so a missed run coalesces into a single refetch
/// of each side). Exits when the engine — and thus the broadcast — is gone. Returns
/// the task handle (used by tests; the production caller fire-and-forgets it).
fn spawn_spine_changed_forwarder(
    mut spine_rx: tokio::sync::broadcast::Receiver<SpineChange>,
    bus: Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match spine_rx.recv().await {
                Ok(SpineChange::Projects) => bus.emit(projects_changed_event()),
                Ok(SpineChange::Sessions) => bus.emit(sessions_changed_event()),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    bus.emit(projects_changed_event());
                    bus.emit(sessions_changed_event());
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// Max topics a single `/ws/events` subscribe frame may carry (reject the frame
/// beyond this) and max total fine topics one connection may hold.
const MAX_EVENT_TOPICS_PER_FRAME: usize = 64;
const MAX_EVENT_TOPICS_PER_CONN: usize = 64;

/// Max length (chars) of a single topic string. A topic that exceeds this is
/// ignored before it is inserted into the set or used for a `session_worktree`
/// round-trip, so a client cannot push huge strings into the per-connection set or
/// trigger expensive lookups with them.
const MAX_TOPIC_LEN: usize = 256;

/// Inbound `/ws/events` control frame: subscribe and/or unsubscribe sets. Both
/// arrays are optional so `{ "subscribe": [...] }` and `{ "unsubscribe": [...] }`
/// each parse, and a frame may carry both.
#[derive(serde::Deserialize)]
struct EventsClientFrame {
    #[serde(default)]
    subscribe: Vec<String>,
    #[serde(default)]
    unsubscribe: Vec<String>,
}

/// Outbound `/ws/events` resource-change signal. Mirrors the event envelope:
/// `{ "event": "session.changes", "id": "s1", "rev": 42 }`. Also carries the
/// `connected` handshake (`{ "event": "connected", "id": "<conn>" }`).
#[derive(serde::Serialize)]
struct WireEvent {
    event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rev: Option<u64>,
    /// The claiming connection's id on a `pty.owner` handover (see
    /// [`pty_owner_event`]). Omitted from the wire for every other event.
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    /// The monotonic ownership epoch on a `pty.owner` handover (see
    /// [`pty_owner_event`]). The client keeps only the highest epoch seen per pty
    /// and ignores any older arrival, so a reordered broadcast cannot resurrect a
    /// stale owner. Omitted from the wire for every other event.
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch: Option<u64>,
    /// The claiming connection's raw `User-Agent` on a `pty.owner` handover (see
    /// [`pty_owner_event`]), captured server-side. The client parses it into a
    /// human label for the take-over placeholder. Omitted from the wire for every
    /// other event and when the claimer sent no `User-Agent`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    device: Option<String>,
}

/// Process-monotonic id stamped on every PTY scrollback replay (see the
/// `connected`-frame send in [`handle_pty_socket`]). Global rather than per-PTY on
/// purpose: the client only ever compares generations WITHIN one socket's lifetime
/// (drop a replay whose generation it already applied), and a single global counter
/// makes each open strictly newer than any earlier one with no per-target
/// bookkeeping. Starts at 1 so 0 can never be confused with "unset".
static PTY_REPLAY_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Allocate the next replay generation. `Relaxed` is sufficient: the value only has
/// to be unique and monotonically increasing per allocation, not synchronized with
/// any other memory.
fn next_replay_generation() -> u64 {
    PTY_REPLAY_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The opening handshake frame on a PTY socket:
/// `{event:"connected", id, gen, owner}`. A superset of the events-socket
/// `connected` frame (which is a [`WireEvent`] and carries no generation): a PTY
/// socket also stamps the scrollback replay that follows with its `gen` so the
/// client can drop an already-applied replay, and names the pty's current
/// `owner` so the arriving client knows whether it is joining as the driver or
/// as a watcher. Adding a field is backward-safe (an older client ignores both).
#[derive(serde::Serialize)]
struct PtyConnectedFrame {
    event: &'static str,
    id: String,
    /// The generation of the replay Binary frame that follows this handshake.
    /// Serialized as `gen` on the wire (`gen` is a reserved keyword in the Rust
    /// 2024 edition, so the field is spelled out and renamed for JSON).
    #[serde(rename = "gen")]
    generation: u64,
    /// Who currently owns sizing+input for this pty: the owning connection's id,
    /// or `null` when nobody is driving it. Deliberately serialized even when
    /// null, so the field's PRESENCE tells the client it is talking to a server
    /// that answers the question at all; a client that finds it absent falls back
    /// to the old foreground guess rather than assuming an unowned pty.
    owner: Option<String>,
    /// The ownership epoch as of the same owners-lock read that produced
    /// `owner`, the SAME counter every `pty.owner` broadcast is stamped with.
    /// Named `owner_epoch` rather than a bare `epoch` because this frame already
    /// carries a second counter (`gen`, the replay generation) and the two must
    /// not be mistaken for each other. Always serialized alongside `owner`: the
    /// handshake and the `pty.owner` broadcasts travel on different sockets, so
    /// a client that has already applied a strictly newer `pty.owner` uses this
    /// to recognize the handshake's owner snapshot as stale and keep the newer
    /// verdict. An old server omits both keys together, which is the client's
    /// mixed-version fallback signal.
    owner_epoch: u64,
    /// The owner's device label: the raw `User-Agent` the owning connection
    /// presented at its upgrade, recorded at claim time and read here under the
    /// SAME owners-lock acquisition as `owner` and `owner_epoch`. It is the
    /// same string the claim's own `pty.owner` broadcast carried as `device`,
    /// and it exists because a mere attach emits no such broadcast: without it
    /// a watcher that simply opened the pane could only title its take-over
    /// card "Active on another device". Unlike `owner`, absence needs no
    /// second meaning (the owner key already tells an old server apart), so it
    /// is omitted rather than null when there is no owner or the owner sent no
    /// `User-Agent`; a client treats an absent key as "no name known" and
    /// falls back to the generic title.
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_device: Option<String>,
    /// The PTY's grid at attach time, the geometry the child is drawing for.
    /// Deliberately serialized even when null, for the same reason `owner` is:
    /// the field's PRESENCE tells the client this server answers the question
    /// at all, and a client that finds it absent knows nothing about the grid
    /// rather than assuming it agrees. Null is the honest answer when the pty
    /// could not be read (it is not running, or its terminal lock is poisoned);
    /// inventing a size would make a viewer certain it agreed when it did not.
    ///
    /// This is the attach-time snapshot ONLY. Later changes arrive as `size`
    /// event frames on this same socket (see [`PtySizeFrame`]), and a client
    /// deliberately treats the two differently: the handshake is what its fresh
    /// attach is already sized against, while a later change is what makes a
    /// viewer re-attach to heal.
    rows: Option<u16>,
    cols: Option<u16>,
    /// The per-pty grid sequence as of this handshake, the SAME counter every
    /// `size` event's `seq` is drawn from (stamped under the owners lock in
    /// apply order; see [`PtySizeFrame::seq`]). Read server-side BEFORE the
    /// grid above, so it is a lower bound for what `rows`/`cols` reflect: the
    /// client seeds its last-seen seq from it and drops any `size` event at or
    /// below it, which is what stops a stale broadcast, buffered on this socket
    /// from before the handshake, from regressing the grid after the attach.
    /// Named `grid_seq` rather than a bare `seq` because this frame already
    /// carries two other counters (`gen` and `owner_epoch`).
    grid_seq: u64,
}

/// A grid-change event frame pushed to every socket attached to one PTY:
/// `{"event":"size","rows":R,"cols":C}`. Named `event` to match the `connected`
/// handshake beside it on this socket and the `pty.owner` events on
/// `/ws/events`, so one client-side parse can tell the frames apart by that key
/// alone.
///
/// Emitted only where a resize was really APPLIED to the child. A refused
/// resize (a non-owner's plain frame) changed nothing and says nothing, exactly
/// like the `pty.owner` broadcast beside it, or every viewer would be told the
/// grid had moved to a size the PTY never took.
#[derive(serde::Serialize)]
struct PtySizeFrame {
    event: &'static str,
    rows: u16,
    cols: u16,
    /// The per-pty grid sequence, stamped by `claim_for_resize` under the
    /// owners lock in apply order, exactly as `epoch` is documented on
    /// `pty.owner`: the broadcasts behind these frames are emitted AFTER the
    /// lock releases and the runtime may reorder two near-simultaneous ones,
    /// so the client keeps only the highest seq seen per socket (seeded from
    /// the handshake's `grid_seq`) and ignores any older arrival, and a stale
    /// announcement can never become its last word on the grid.
    seq: u64,
}

/// Serialize a [`PtySizeFrame`] for the wire. Falls back to an empty string on
/// the impossible serialization failure; the caller skips an empty frame.
fn pty_size_frame_text(rows: u16, cols: u16, seq: u64) -> String {
    serde_json::to_string(&PtySizeFrame {
        event: "size",
        rows,
        cols,
        seq,
    })
    .unwrap_or_default()
}

/// The answer to a [`PtyBeatFrame`]: `{"event":"beat","n":N}`, echoing the
/// client's own number so an answer to a stale beat can never be counted as an
/// answer to the current one. Keyed by `event` like every other Text frame this
/// socket sends, so one client-side parse tells them all apart.
///
/// Built by hand rather than through a serde struct because it has exactly two
/// fields and one of them is a literal; the unit test pins the bytes.
fn pty_beat_frame_text(n: u64) -> String {
    format!(r#"{{"event":"beat","n":{n}}}"#)
}

/// Serialize and send the PTY-socket `connected` handshake carrying this socket's
/// connection id, the replay generation for the repaint that follows, the
/// pty's current owner plus the ownership epoch and the owner's device label of
/// that snapshot (all three read under ONE owners-lock acquisition by the
/// caller), the grid the child is currently drawing for, and the grid sequence
/// that grid is at least as new as (see [`PtyConnectedFrame::grid_seq`]).
///
/// `owner_snapshot` is [`PtySizeOwners::current_owner`]'s answer verbatim, kept
/// as one value so the three fields that were read under one lock acquisition
/// travel together and cannot be recombined from different snapshots.
async fn send_pty_connected(
    sink: &SharedSink,
    conn_id: u64,
    generation: u64,
    owner_snapshot: (Option<u64>, u64, Option<String>),
    grid: Option<(u16, u16)>,
    grid_seq: u64,
) -> Result<(), ()> {
    let (owner, owner_epoch, owner_device) = owner_snapshot;
    let frame = PtyConnectedFrame {
        event: "connected",
        id: conn_id.to_string(),
        generation,
        owner: owner.map(|id| id.to_string()),
        owner_epoch,
        owner_device,
        rows: grid.map(|(rows, _)| rows),
        cols: grid.map(|(_, cols)| cols),
        grid_seq,
    };
    let text = serde_json::to_string(&frame).map_err(|_| ())?;
    let mut guard = sink.lock().await;
    guard.send(Message::Text(text.into())).await.map_err(|_| ())
}

/// Outbound `/ws/events` status event: the one event carrying an inline payload
/// (a toast has nothing to GET). Shape:
/// `{ "event": "status", "key": "op-7", "tone": "info", "message": "…", "scope": "all",
/// "sticky": false }`.
/// The server has already filtered on `scope`, but it is serialized for wire
/// parity so a client may render/correlate it. `sticky` tells the client to hold
/// the toast until the user dismisses it instead of retiring it on a timer; see
/// [`dux_core::statusline::KeyedWireStatus::sticky`].
#[derive(serde::Serialize)]
struct WireStatusEvent {
    event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    tone: String,
    message: String,
    scope: StatusScope,
    sticky: bool,
}

/// Outbound `/ws/events` status-clear event: dismiss the toast for `key` (a keyed
/// op resolved or was cleared). `None` clears the anonymous slot. Shape:
/// `{ "event": "status_cleared", "key": "op-7" }`.
#[derive(serde::Serialize)]
struct WireStatusClearedEvent {
    event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

/// Upgrade handler for `/ws/events`. Replicates the WS protections (origin
/// check, connection-cap permit, frame-size limit). The per-connection
/// `connection_id` (minted inside [`handle_events_socket`]) is sent as the first
/// frame and drives status-toast scoping: a REST action echoes it via
/// `X-Connection-Id` so its status reaches only the originating connection.
async fn ws_events_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !same_origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin WebSocket upgrade rejected",
        )
            .into_response();
    }
    let permit = match acquire_ws_permit(
        &state.ws_events_semaphore,
        peer.ip(),
        "/ws/events",
        "max_websocket_events_connections",
    ) {
        Some(permit) => permit,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "too many WebSocket connections; try again shortly",
            )
                .into_response();
        }
    };
    let console = state.console.clone();
    let engine = state.engine.clone();
    let bus = Arc::clone(&state.event_bus);
    let changes = Arc::clone(&state.changes);
    let connections = Arc::clone(&state.connections);
    let peer_ip = peer.ip();
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_events_socket(
                socket,
                engine,
                bus,
                changes,
                console,
                peer_ip,
                permit,
                connections,
            )
        })
        .into_response()
}

/// Drive one `/ws/events` connection as a single `tokio::select!` loop owning the
/// subscription `HashSet` and the only path that drains held interests on exit.
/// There is no separate forwarder task (no double-decrement, no forwarder-dies-
/// but-handler-lives leak).
///
/// Besides resource-change events, this socket also delivers status toasts: the
/// live status broadcast, the status-clear broadcast, and the on-connect status
/// snapshot — all filtered by the per-connection scope rule ([`scope_delivers`])
/// so one client's operation toasts never leak to another.
#[allow(clippy::too_many_arguments)]
async fn handle_events_socket(
    socket: WebSocket,
    engine: EngineHandle,
    bus: Arc<EventBus>,
    changes: Arc<ChangesService>,
    console: Console,
    peer_ip: std::net::IpAddr,
    _permit: tokio::sync::OwnedSemaphorePermit,
    connections: Arc<crate::rest_common::ConnectionRegistry>,
) {
    console.client_connected(peer_ip);
    // A server-assigned random id correlating REST actions with the statuses they
    // mint, so an operation's toasts (push/commit/launch) are delivered ONLY back
    // to the originating connection (`StatusScope::Connection`). The client echoes
    // it as the `X-Connection-Id` header on REST mutations. Never client-supplied.
    let connection_id = uuid::Uuid::new_v4().to_string();
    // Register this id as a live connection so `scope_from_headers` validates the
    // echoed `X-Connection-Id` against it. The guard deregisters on EVERY exit path
    // (loop break or task cancellation), freeing the slot.
    connections.insert(connection_id.clone(), crate::rest_common::ConnClass::Events);
    let _conn_guard = ConnectionGuard {
        id: connection_id.clone(),
        registry: Arc::clone(&connections),
    };
    let (sink, mut stream) = socket.split();
    let sink: SharedSink = Arc::new(tokio::sync::Mutex::new(sink));
    let mut bus_rx = bus.subscribe();

    // First frame: hand the client its connection id (the `X-Connection-Id` REST
    // mutations echo back so their status toasts scope to this connection only).
    let _ = send_event(
        &sink,
        &WireEvent {
            event: "connected".to_string(),
            id: Some(connection_id.clone()),
            rev: None,
            owner: None,
            epoch: None,
            device: None,
        },
    )
    .await;

    // Subscribe to the live status + status-clear broadcasts BEFORE reading the
    // snapshot: the broadcast does not replay to a receiver created after a send,
    // so a status/clear emitted in the gap (notably during a snapshot
    // `send_event().await`) would be lost. Subscribing first buffers it for this
    // receiver; any overlap with the snapshot is a harmless duplicate.
    let mut status_rx = engine.subscribe_status();
    let mut status_clear_rx = engine.subscribe_status_clears();

    // The pushed workspace document. A cloned `watch` receiver copies the
    // SOURCE receiver's seen-version (the handle's long-lived receiver, which
    // never reads), so the first `changed()` here fires immediately with the
    // current document rather than waiting for a new one. That is harmless by
    // construction: with no coarse topic held yet the frame is filtered, and
    // once one is held the replay/dedup-by-rev absorbs the duplicate. `workspace_alive` retires the arm if the engine goes away: a
    // `watch` whose sender is dropped returns `Err` from `changed()` forever,
    // which would spin this select loop.
    let mut workspace_rx = engine.workspace_docs();
    let mut workspace_alive = true;

    // Initial statuses: a client connecting mid-operation sees ALL active toasts
    // (keyed and anonymous) immediately, scoped to itself. An empty/fully-filtered
    // snapshot sends nothing.
    for ev in status_events(&engine.status_snapshot(), &connection_id) {
        if send_status_event(&sink, &ev).await.is_err() {
            console.client_disconnected(peer_ip);
            return;
        }
    }

    // This connection's fine + coarse topic set (the sole owner), wrapped in a Drop
    // guard so the held fine-topic interests are drained on EVERY exit — including
    // task cancellation (a runtime shutdown drops this future at an `.await`), not
    // just the normal loop break. Leaking interest would keep the poller computing
    // for a gone connection forever.
    let mut interest = InterestGuard {
        subscribed: std::collections::HashSet::new(),
        bus: Arc::clone(&bus),
    };

    // Liveness ping (every connection). The first interval tick fires immediately;
    // consume it so the first real ping waits a full period.
    let mut ping = tokio::time::interval(WS_LIVENESS_PING_PERIOD);
    ping.tick().await;

    loop {
        tokio::select! {
            // Liveness ping: a failed send means a dead/half-open peer — break so
            // the socket tears down and its permit + registry slot are freed.
            _ = ping.tick() => {
                if send_ping(&sink).await.is_err() {
                    break;
                }
            }
            // The workspace document changed: push it to this connection if it
            // asked for the coarse topics. Every events socket wakes once per
            // change and then filters, which is bounded by the connection cap
            // and far cheaper than the N full GETs it replaces.
            changed = workspace_rx.changed(), if workspace_alive => match changed {
                Ok(()) => {
                    // `borrow_and_update`, not `borrow`: `borrow` leaves this
                    // receiver marked as unseen, so `changed()` would return
                    // immediately forever and hot-loop a connection that holds
                    // neither coarse topic.
                    let doc = workspace_rx.borrow_and_update().clone();
                    if let Some(doc) = doc
                        && holds_workspace_topic(&interest.subscribed)
                        && send_text(&sink, workspace_frame_text(&doc)).await.is_err()
                    {
                        break;
                    }
                }
                // The engine is gone. The status broadcasts will close too and
                // break this loop; retire the arm so it cannot spin first.
                Err(_) => workspace_alive = false,
            },
            ev = bus_rx.recv() => match ev {
                Ok(Event::Resource {
                    event,
                    id,
                    rev,
                    owner,
                    epoch,
                    device,
                }) => {
                    // Forward a resource event only if this connection holds the
                    // topic it is delivered on. `session.changes` rides the fine
                    // per-session `session:<id>:changes` topic; `config.changed`
                    // rides the coarse `config` topic (no id/rev — a plain refetch
                    // signal for `/api/v1/bootstrap`).
                    let deliver = match (event.as_str(), &id) {
                        ("session.changes", Some(sid)) => {
                            interest.subscribed.contains(&event_bus::changes_topic(sid))
                        }
                        ("config.changed", _) => interest.subscribed.contains("config"),
                        // Coarse workspace signals ride their own coarse topics
                        // (no id/rev). The document they announce is pushed on
                        // the watch arm above; these remain for a page too old
                        // to read that, which refetches `/api/v1/workspace`.
                        ("projects.changed", _) => interest.subscribed.contains("projects"),
                        ("sessions.changed", _) => interest.subscribed.contains("sessions"),
                        // A PTY ownership handover rides the coarse `sessions` topic
                        // (held by every client). Coarse delivery is fine: only the
                        // client(s) actually viewing that pty id react to it.
                        ("pty.owner", _) => interest.subscribed.contains("sessions"),
                        _ => false,
                    };
                    if deliver {
                        let frame = WireEvent {
                            event,
                            id,
                            rev,
                            owner,
                            epoch,
                            device,
                        };
                        if send_event(&sink, &frame).await.is_err() {
                            break;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    dux_core::logger::warn(&format!(
                        "WebSocket events client {peer_ip} lagged behind the event bus; \
                         dropped {n} event(s); synthesizing catch-up"
                    ));
                    // Write a synthetic catch-up DIRECTLY to this connection's sink
                    // (never back onto the broadcast bus). The whole set is built by
                    // one shared, tested function; the current workspace document is
                    // read here because a `watch` borrow must not be held across the
                    // sends below.
                    let doc = workspace_rx.borrow_and_update().clone();
                    let mut sink_dead = false;
                    for text in
                        lagged_catchup_texts(&interest.subscribed, &changes, doc.as_deref())
                    {
                        if send_text(&sink, text).await.is_err() {
                            sink_dead = true;
                            break;
                        }
                    }
                    if sink_dead {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            // Live status broadcast. Per-connection scope filter: an `All` status
            // reaches everyone; a `Connection(id)` status reaches only that
            // connection — one client's operation toasts stop leaking to others.
            status = status_rx.recv() => match status {
                Ok(status) => {
                    if scope_delivers(&status.scope, &connection_id) {
                        let ev = WireStatusEvent {
                            event: "status",
                            key: status.key,
                            tone: status.tone,
                            message: status.message,
                            scope: status.scope,
                            sticky: status.sticky,
                        };
                        if send_status_event(&sink, &ev).await.is_err() {
                            break;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    dux_core::logger::warn(&format!(
                        "WebSocket events client {peer_ip} lagged behind the status \
                         broadcast; dropped {n} update(s); resending scoped snapshot"
                    ));
                    // Re-send the current scoped snapshot: every operation still
                    // in flight, plus any outcome recent enough to be inside
                    // `FINAL_REPLAY_WINDOW`. The client replaces its toast per
                    // key, so a dropped update for one of those is healed. It is
                    // NOT a full recovery and must not be described as one: an
                    // outcome older than the window is gone, and so is any
                    // dismissal, since the snapshot carries no way to say that
                    // something ended.
                    if resend_status_snapshot(&sink, &engine, &connection_id)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            // Keyed-status clears: when a keyed op resolves or expires, dismiss the
            // matching toast immediately. `None` clears the anonymous slot.
            cleared = status_clear_rx.recv() => match cleared {
                Ok(key) => {
                    let ev = WireStatusClearedEvent {
                        event: "status_cleared",
                        key,
                    };
                    if send_status_cleared_event(&sink, &ev).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    dux_core::logger::warn(&format!(
                        "WebSocket events client {peer_ip} lagged behind the status-clear \
                         broadcast; dropped {n} clear(s); resending scoped snapshot"
                    ));
                    // A dropped clear cannot be recovered. The client shows a
                    // toast per `status` frame and dismisses on `status_cleared`;
                    // it does NOT reconcile itself to the snapshot as a set, so
                    // re-sending cannot retract a toast whose dismissal was
                    // missed. What the resend does buy is that anything still
                    // open, or finished within `FINAL_REPLAY_WINDOW`, is
                    // re-asserted with its current tone and message, so the
                    // client is at least not left showing a stale spinner for an
                    // operation that has since resolved.
                    if resend_status_snapshot(&sink, &engine, &connection_id)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            next = stream.next() => match next {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(frame) = serde_json::from_str::<EventsClientFrame>(text.as_str()) {
                        let new_topics =
                            apply_events_frame(&frame, &mut interest.subscribed, &engine, &bus)
                                .await;
                        // Replay the current workspace document to a connection
                        // that JUST asked for it, so it starts from the truth
                        // instead of from whatever its boot fetch returned. Skip
                        // it before the engine has published anything (the
                        // pre-first-build `None`): there is nothing truthful to
                        // send, and the client's boot fetch covers that window.
                        if new_topics.workspace {
                            let doc = workspace_rx.borrow_and_update().clone();
                            if let Some(doc) = doc
                                && send_text(&sink, workspace_frame_text(&doc)).await.is_err()
                            {
                                break;
                            }
                        }
                        let new_fine = new_topics.fine;
                        // Per-subscribe catch-up: for each newly-registered fine
                        // topic, send a `session.changes` frame immediately so the
                        // client does not miss an event that landed between its REST
                        // refetch and this subscription registering. `peek_rev`
                        // returns `None` for a cold cache, which serialises to an
                        // absent `rev` field; the client treats that as a
                        // force-refetch — correct.
                        let mut sink_dead = false;
                        for frame in catchup_frames(&new_fine, &changes) {
                            if send_event(&sink, &frame).await.is_err() {
                                sink_dead = true;
                                break;
                            }
                        }
                        if sink_dead {
                            break;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                // Ignore binary/ping/pong on the events socket.
                Some(Ok(_)) => {}
            },
        }
    }

    // `interest` (the Drop guard) drains all held fine-topic interests when it
    // goes out of scope here — on the normal break above AND on task cancellation.
    drop(interest);
    console.client_disconnected(peer_ip);
}

/// Drains a `/ws/events` connection's held fine-topic interests on Drop, so the
/// global poll-interest refcount is balanced on EVERY exit path — the normal loop
/// break and task cancellation alike (a runtime shutdown drops the connection
/// future at an `.await`, which would otherwise skip a hand-written cleanup at the
/// end of the function). Holds an `Arc<EventBus>` clone so the bus outlives it.
struct InterestGuard {
    subscribed: std::collections::HashSet<String>,
    bus: Arc<EventBus>,
}

impl Drop for InterestGuard {
    fn drop(&mut self) {
        for topic in &self.subscribed {
            if event_bus::session_id_from_changes_topic(topic).is_some() {
                self.bus.drop_interest(topic);
            }
        }
    }
}

/// Apply one subscribe/unsubscribe frame to the connection's topic set, keeping
/// the global interest refcount exact (`add_interest` only on a genuine insert,
/// `drop_interest` only on a genuine removal). Validates a `session:<id>:changes`
/// subscription against a live session before registering interest, and enforces
/// the per-frame and per-connection topic caps.
///
/// Returns what this frame newly registered (see [`NewSubscriptions`]): the
/// fine `session:<id>:changes` topics, so the caller can send one
/// `session.changes` catch-up frame per newly-subscribed session, and whether a
/// coarse workspace topic was newly registered, so the caller can replay the
/// current workspace document. Both close the same race window: the gap between
/// a client's REST read and its subscription registering.
async fn apply_events_frame(
    frame: &EventsClientFrame,
    subscribed: &mut std::collections::HashSet<String>,
    engine: &EngineHandle,
    bus: &EventBus,
) -> NewSubscriptions {
    // Process unsubscribes FIRST — they only ever shrink state, so they are always
    // safe to honor (even on an otherwise-rejected oversized frame) and a frame
    // carrying both makes room under the cap before the subscribes run.
    for topic in &frame.unsubscribe {
        if subscribed.remove(topic) && event_bus::session_id_from_changes_topic(topic).is_some() {
            bus.drop_interest(topic);
        }
    }

    // Only AFTER honoring unsubscribes, reject an oversized subscribe set.
    if frame.subscribe.len() > MAX_EVENT_TOPICS_PER_FRAME {
        dux_core::logger::warn(&format!(
            "/ws/events subscribe frame rejected: {} topics exceeds the {MAX_EVENT_TOPICS_PER_FRAME} cap",
            frame.subscribe.len()
        ));
        return NewSubscriptions::default();
    }

    let mut new = NewSubscriptions::default();

    for topic in &frame.subscribe {
        if subscribed.len() >= MAX_EVENT_TOPICS_PER_CONN {
            dux_core::logger::warn(&format!(
                "/ws/events connection hit the {MAX_EVENT_TOPICS_PER_CONN}-topic cap; \
                 ignoring further subscriptions"
            ));
            break;
        }
        // Bound a single topic's length before inserting it or using it for a
        // (possibly expensive) session lookup.
        if topic.chars().count() > MAX_TOPIC_LEN {
            dux_core::logger::debug(&format!(
                "/ws/events ignoring an over-long topic ({} chars exceeds {MAX_TOPIC_LEN})",
                topic.chars().count()
            ));
            continue;
        }
        match event_bus::session_id_from_changes_topic(topic) {
            // A fine session-changes topic.
            Some(sid) => {
                // Already held → O(1), skip the `session_worktree` round-trip.
                if subscribed.contains(topic) {
                    continue;
                }
                // Validate the session exists before registering interest; drop a
                // phantom-session subscription with a breadcrumb (the other
                // rejections log, so this one shouldn't be silent).
                if engine.session_worktree(sid.to_string()).await.is_none() {
                    dux_core::logger::debug(&format!(
                        "/ws/events ignoring subscription to unknown session {sid:?}"
                    ));
                    continue;
                }
                if subscribed.insert(topic.clone()) {
                    bus.add_interest(topic);
                    // Collect for the per-subscribe catch-up emitted at the
                    // caller, closing the race window between a REST refetch
                    // and the subscription registering.
                    new.fine.push(topic.clone());
                }
            }
            // A coarse topic (sessions/projects/config): tracked for forwarding,
            // but it carries no poll interest.
            None => {
                if subscribed.insert(topic.clone()) && (topic == "sessions" || topic == "projects")
                {
                    new.workspace = true;
                }
            }
        }
    }

    new
}

/// What one subscribe frame newly registered, and therefore what the connection
/// must be caught up on before it can trust what it holds.
#[derive(Default)]
struct NewSubscriptions {
    /// Newly inserted `session:<id>:changes` fine topics.
    fine: Vec<String>,
    /// Whether a coarse workspace topic (`sessions`/`projects`) was newly
    /// inserted, so the current workspace document should be replayed. A
    /// re-subscribe to a topic already held is deliberately not a replay: the
    /// connection is already being pushed every change to it.
    workspace: bool,
}

/// Build the set of per-subscribe catch-up frames for `new_fine`: for each newly
/// inserted `session:<id>:changes` fine topic, parse the session id and read the
/// current cached rev from `changes`. Returns one [`WireEvent`] per topic; topics
/// that do not parse as a changes topic (should not happen in practice) are silently
/// skipped.
///
/// This is the SHARED production+test path for the catch-up emit, so tests exercise
/// the topic-parse + rev-read integration rather than just serialization.
fn catchup_frames(new_fine: &[String], changes: &ChangesService) -> Vec<WireEvent> {
    new_fine
        .iter()
        .filter_map(|topic| {
            let sid = event_bus::session_id_from_changes_topic(topic)?;
            Some(WireEvent {
                event: "session.changes".to_string(),
                id: Some(sid.to_string()),
                rev: changes.peek_rev(sid),
                owner: None,
                epoch: None,
                device: None,
            })
        })
        .collect()
}

/// Every catch-up frame a connection that lagged the event bus needs, in send
/// order, already serialized.
///
/// A lagged connection missed an unknown number of events, so recovery is "here
/// is where everything stands": one `session.changes` per held fine topic
/// (carrying the current rev), one refetch nudge per held coarse topic, and the
/// current workspace document itself.
///
/// The document is sent ALONGSIDE the `projects.changed`/`sessions.changed`
/// nudges rather than instead of them. A client that does not understand the
/// pushed document still needs its nudge, and one that does simply discards the
/// second copy: its revision is not newer than the one it just applied. Paying
/// for one redundant frame on a rare lag is much cheaper than deciding, per
/// connection, which kind of client is on the other end.
///
/// This is the SHARED production+test path, like [`catchup_frames`], so the
/// tests exercise the real assembly rather than a re-description of it.
fn lagged_catchup_texts(
    subscribed: &std::collections::HashSet<String>,
    changes: &ChangesService,
    workspace: Option<&WorkspaceDoc>,
) -> Vec<String> {
    let mut events: Vec<WireEvent> = Vec::new();
    for topic in subscribed {
        if let Some(sid) = event_bus::session_id_from_changes_topic(topic) {
            events.push(WireEvent {
                event: "session.changes".to_string(),
                id: Some(sid.to_string()),
                rev: changes.peek_rev(sid),
                owner: None,
                epoch: None,
                device: None,
            });
        }
    }
    // The coarse topics carry no per-resource rev, so the fine-topic loop above
    // never covers them. A lagged client holding `config` would keep a stale
    // bootstrap, and one holding `projects`/`sessions` a stale workspace, unless
    // told explicitly to refetch (mirroring how the forwarders recover).
    for (topic, event) in [
        ("config", "config.changed"),
        ("projects", "projects.changed"),
        ("sessions", "sessions.changed"),
    ] {
        if subscribed.contains(topic) {
            events.push(WireEvent {
                event: event.to_string(),
                id: None,
                rev: None,
                owner: None,
                epoch: None,
                device: None,
            });
        }
    }
    let mut texts: Vec<String> = events
        .iter()
        .filter_map(|ev| serde_json::to_string(ev).ok())
        .collect();
    if let Some(doc) = workspace
        && holds_workspace_topic(subscribed)
    {
        texts.push(workspace_frame_text(doc));
    }
    texts
}

/// Send one already-serialized `/ws/events` text frame.
async fn send_text(sink: &SharedSink, text: String) -> Result<(), ()> {
    let mut guard = sink.lock().await;
    guard.send(Message::Text(text.into())).await.map_err(|_| ())
}

/// Serialize and send one `/ws/events` resource frame as a text message.
async fn send_event(sink: &SharedSink, ev: &WireEvent) -> Result<(), ()> {
    let text = serde_json::to_string(ev).map_err(|_| ())?;
    let mut guard = sink.lock().await;
    guard.send(Message::Text(text.into())).await.map_err(|_| ())
}

/// Serialize and send one `/ws/events` status event as a text message.
async fn send_status_event(sink: &SharedSink, ev: &WireStatusEvent) -> Result<(), ()> {
    let text = serde_json::to_string(ev).map_err(|_| ())?;
    let mut guard = sink.lock().await;
    guard.send(Message::Text(text.into())).await.map_err(|_| ())
}

/// Serialize and send one `/ws/events` status-clear event as a text message.
async fn send_status_cleared_event(
    sink: &SharedSink,
    ev: &WireStatusClearedEvent,
) -> Result<(), ()> {
    let text = serde_json::to_string(ev).map_err(|_| ())?;
    let mut guard = sink.lock().await;
    guard.send(Message::Text(text.into())).await.map_err(|_| ())
}

/// Whether a status of the given [`StatusScope`] is delivered to the connection
/// with id `conn_id`: `All` reaches everyone; `Connection(id)` reaches only the
/// matching connection. Shared by the live status arm and the on-connect snapshot
/// so both delivery paths filter identically.
fn scope_delivers(scope: &StatusScope, conn_id: &str) -> bool {
    match scope {
        StatusScope::All => true,
        StatusScope::Connection(id) => id == conn_id,
    }
}

/// Re-send the current scoped status snapshot to one connection, after a
/// `Lagged` on either the status or the status-clear broadcast.
///
/// Be precise about what this does and does not fix, because the previous
/// wording promised more than the code delivers. It re-asserts every operation
/// still IN FLIGHT plus any final still inside `FINAL_REPLAY_WINDOW`, each
/// replacing the client's toast for that key. It does NOT recover an outcome
/// older than the window, and it does NOT recover a missed DISMISSAL: the client
/// adds a toast per `status` frame and removes one on `status_cleared`, it never
/// reconciles itself to the snapshot as a set, so nothing the server re-sends can
/// retract a toast. Returns `Err(())` if the sink is dead so the caller can break
/// the connection loop.
async fn resend_status_snapshot(
    sink: &SharedSink,
    engine: &EngineHandle,
    connection_id: &str,
) -> Result<(), ()> {
    for ev in status_events(&engine.status_snapshot(), connection_id) {
        send_status_event(sink, &ev).await?;
    }
    Ok(())
}

/// Build the status events to replay on connect from a status snapshot.
///
/// Each `KeyedWireStatus` in `snapshot` (non-empty message) whose scope is
/// deliverable to `conn_id` maps to one [`WireStatusEvent`]. The snapshot holds
/// operations still in flight plus finals still inside `FINAL_REPLAY_WINDOW`, so
/// this is what tells a mid-operation joiner about running work AND what tells a
/// reconnecting tab how the operation it was watching ended. The scope filter
/// mirrors the live status arm so a client connecting mid-operation does NOT
/// receive another connection's in-progress `Busy` (a ghost spinner that never
/// clears). Pure and side-effect-free so it can be unit-tested without a
/// WebSocket. An empty (or fully-filtered) snapshot produces an empty `Vec`.
fn status_events(snapshot: &[KeyedWireStatus], conn_id: &str) -> Vec<WireStatusEvent> {
    snapshot
        .iter()
        .filter(|e| !e.message.is_empty())
        .filter(|e| scope_delivers(&e.scope, conn_id))
        .map(|e| WireStatusEvent {
            event: "status",
            key: e.key.clone(),
            tone: e.tone.clone(),
            message: e.message.clone(),
            scope: e.scope.clone(),
            sticky: e.sticky,
        })
        .collect()
}

/// Send one Binary frame. `Err(())` when the send fails, exactly like
/// [`send_text`] and [`send_ping`], so a caller for whom a failed send strands
/// the client can act on it. Callers that legitimately do not care discard the
/// result.
async fn send_binary(sink: &SharedSink, bytes: Vec<u8>) -> Result<(), ()> {
    let mut guard = sink.lock().await;
    guard
        .send(Message::Binary(bytes.into()))
        .await
        .map_err(|_| ())
}

/// Resolve the deadline for a PTY socket's OPENING sends (the `connected`
/// handshake and the scrollback replay), from `[server]
/// pty_send_timeout_seconds`.
///
/// WHY THERE IS A BOUND AT ALL. An unbounded send against a wedged socket never
/// returns, and it holds the sink lock while it waits, so nothing else on that
/// socket can write either. The client is then left in front of a permanently
/// blank terminal pane with a connection that looks alive from both ends, which
/// is the exact bug report this exists to answer.
///
/// WHY IT IS CONFIGURABLE, AND GENEROUS. A send completes when the bytes reach
/// the peer, so on a degraded link this measures THROUGHPUT and not liveness,
/// and what it is measuring is the whole scrollback replay
/// (`agent_scrollback_lines` defaults to 10000 lines and may be set as high as
/// 100000). At cellular speeds a fixed ten seconds was exceeded
/// DETERMINISTICALLY on a large buffer, and the client then retried forever,
/// rebuilding and re-sending the same replay each time: a terminal that could
/// never attach at all. A minute is far longer than any healthy send of that
/// needs and still far shorter than forever, and a user on a genuinely slow link
/// can raise it.
///
/// A zero or missing value falls back to the compiled default rather than
/// removing the bound: no bound is the one answer this must not give.
fn pty_opening_send_timeout(limits: &crate::engine_actor::LiveServerLimits) -> std::time::Duration {
    let seconds = limits.pty_send_timeout_seconds();
    let seconds = if seconds == 0 {
        dux_core::config::DEFAULT_PTY_SEND_TIMEOUT_SECONDS as usize
    } else {
        seconds
    };
    std::time::Duration::from_secs(seconds as u64)
}

/// The deadline for the BEAT ECHO, and the reason it is not the one above.
///
/// The two sends are measuring different things. An opening send is a whole
/// scrollback replay, so its deadline is a THROUGHPUT allowance and has to be
/// generous enough for a hundred thousand lines over a cellular link. The echo
/// is roughly twenty-five bytes; nothing about a healthy link takes seconds to
/// put those on the wire, so its deadline is a LIVENESS check and wants to be
/// short.
///
/// It matters because this send holds the shared sink lock while it waits. On
/// the generous deadline, one wedged peer parks every other write on that socket
/// (a `size` event, the next replay) behind a beat answer the browser has long
/// since stopped waiting for: the client's own answer deadline defaults to
/// thirty seconds and it drops the socket when it passes. Waiting a minute to
/// answer a question nobody is still asking is the worst of both.
///
/// A compile-time constant rather than a setting, deliberately: it is a bound on
/// a fixed twenty-five byte write, so there is no link slow enough to make
/// raising it the right answer, and the configurable knob beside it already
/// covers the send whose size a user can actually change.
const PTY_BEAT_ECHO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Bound one send by `deadline`, flattening "timed out" and "the send failed"
/// into the one verdict the caller acts on: give up on this socket. Kept as a
/// tiny generic wrapper so the deadline can be unit-tested without a live
/// WebSocket, which the test harness cannot stall.
async fn with_send_deadline<F>(deadline: std::time::Duration, send: F) -> Result<(), ()>
where
    F: std::future::Future<Output = Result<(), ()>>,
{
    match tokio::time::timeout(deadline, send).await {
        Ok(result) => result,
        Err(_) => Err(()),
    }
}

/// Send one WebSocket Ping frame on `sink` for the liveness reaper. `Err(())` when
/// the send fails (a dead/half-open peer or an already-closed socket), so the
/// caller breaks its loop and the socket tears down (freeing its permit + registry
/// slot). The peer auto-responds with a Pong at the protocol layer; we do not read
/// the Pong (send-failure reap — see [`WS_LIVENESS_PING_PERIOD`]).
async fn send_ping(sink: &SharedSink) -> Result<(), ()> {
    let mut guard = sink.lock().await;
    guard
        .send(Message::Ping(Vec::new().into()))
        .await
        .map_err(|_| ())
}

/// Removes a live connection's id from the [`ConnectionRegistry`] on Drop, so the
/// id is deregistered on EVERY socket exit path — the normal loop break AND task
/// cancellation (a runtime shutdown drops the socket future at an `.await`). Mirrors
/// the `InterestGuard` pattern. Holds an `Arc` clone of the registry so it outlives
/// the socket task.
struct ConnectionGuard {
    id: String,
    registry: Arc<crate::rest_common::ConnectionRegistry>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.registry.remove(&self.id);
    }
}

/// Releases this PTY socket's sizing/input ownership on Drop, for the same reason
/// [`ConnectionGuard`] deregisters on Drop: a socket that leaves by any path
/// other than the normal loop break (a panic unwinding through the handler, task
/// cancellation at an `.await` during a runtime shutdown) would otherwise leave
/// the pty recorded to a connection that no longer exists, wedging it behind a
/// client that can never see it for the rest of the process's life.
///
/// A release that really cleared an owner is BROADCAST as an owner-cleared
/// `pty.owner`. Ownership no longer follows focus, so nothing else would ever
/// tell the other devices that the driver has gone: their "Active on another
/// device" card would stay up, naming a browser tab that closed, until somebody
/// pressed Take over. Nobody claims the freed pty passively; the broadcast only
/// re-titles the card.
struct PtyOwnershipGuard {
    pty_id: String,
    conn_id: u64,
    owners: Arc<PtySizeOwners>,
    bus: Arc<EventBus>,
}

impl Drop for PtyOwnershipGuard {
    fn drop(&mut self) {
        let Some(epoch) = self.owners.release(&self.pty_id, self.conn_id) else {
            return;
        };
        dux_core::logger::info(&crate::pty_log::describe_ownership_released(
            &self.pty_id,
            self.conn_id,
            epoch,
        ));
        self.bus.emit(pty_owner_cleared_event(&self.pty_id, epoch));
    }
}

/// Decrements the console's live-client count on Drop, so the number a serving
/// dux prints cannot drift upwards over a process's life. Same reasoning as its
/// two siblings above: the increment happens on one line at the top of the
/// handler, and every exit path owes the matching decrement.
struct ClientCountGuard {
    console: Console,
    peer_ip: std::net::IpAddr,
}

impl Drop for ClientCountGuard {
    fn drop(&mut self) {
        self.console.client_disconnected(self.peer_ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::pty_owners::WriteClaim;
    use tower::ServiceExt; // for `oneshot`

    #[test]
    fn captured_user_agent_truncates_long_and_preserves_short() {
        // A short UA is threaded through unchanged.
        let mut short = HeaderMap::new();
        short.insert(
            axum::http::header::USER_AGENT,
            "Mozilla/5.0 Chrome/120".parse().unwrap(),
        );
        assert_eq!(
            captured_user_agent(&short).as_deref(),
            Some("Mozilla/5.0 Chrome/120"),
        );

        // A pathologically long UA is capped to the char bound (char-safe, so the
        // result length is measured in chars, never bytes).
        let mut long = HeaderMap::new();
        let huge = "A".repeat(5000);
        long.insert(axum::http::header::USER_AGENT, huge.parse().unwrap());
        let got = captured_user_agent(&long).expect("header present");
        // Char-count (not byte-count) equals the cap: truncation is char-safe.
        assert_eq!(got.chars().count(), MAX_CAPTURED_USER_AGENT_CHARS);

        // An absent header yields None.
        assert_eq!(captured_user_agent(&HeaderMap::new()), None);
    }

    #[test]
    fn provider_gone_close_carries_the_agreed_code() {
        // The client (`ptySocket.ts`) keys its "stop retrying" behavior off this
        // exact code, so it must stay 4001 and ride a real CloseFrame.
        match provider_gone_close() {
            Message::Close(Some(frame)) => {
                assert_eq!(frame.code, PROVIDER_GONE_CLOSE_CODE);
                assert_eq!(frame.code, 4001);
            }
            other => panic!("expected a Close frame with a code, got {other:?}"),
        }
    }

    #[test]
    fn forwarder_end_close_is_provider_gone_unless_shutting_down() {
        // A provider crash/exit (not shutting down) must tell the client to stop;
        // a shutdown must be a plain close so the client reconnects on restart.
        match forwarder_end_close(false) {
            Message::Close(Some(frame)) => assert_eq!(frame.code, PROVIDER_GONE_CLOSE_CODE),
            other => panic!("expected provider-gone close, got {other:?}"),
        }
        match forwarder_end_close(true) {
            Message::Close(None) => {}
            other => panic!("expected a plain close on shutdown, got {other:?}"),
        }
    }

    /// Boot a minimal headless engine handle for routing-only tests. The handle
    /// just needs to exist: these tests assert on routing and the middleware
    /// (host allowlist / origin check), which answer before the request would
    /// reach the engine.
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

    async fn patch_project_provider(
        app: &Router,
        provider: &str,
    ) -> axum::http::Response<axum::body::Body> {
        app.clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/projects/p1")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(
                        "{{\"provider\":\"{provider}\"}}"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// A project PATCH that sets an UNCONFIGURED provider is rejected up front with
    /// 400 — before any sub-command dispatches — so a bad provider cannot partially
    /// apply after the other fields. A CONFIGURED provider is accepted (200),
    /// proving the guard rejects only the invalid case.
    #[tokio::test]
    async fn project_patch_rejects_unconfigured_provider_up_front() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let app = router(handle);

        let bad = patch_project_provider(&app, "frobnicate").await;
        assert_eq!(
            bad.status(),
            StatusCode::BAD_REQUEST,
            "an unconfigured provider must be rejected up front"
        );
        let body = axum::body::to_bytes(bad.into_body(), 64 * 1024)
            .await
            .unwrap();
        let msg = String::from_utf8_lossy(&body);
        assert!(
            msg.contains("frobnicate") && msg.contains("not configured"),
            "the 400 body should name the bad provider: {msg}"
        );

        let ok = patch_project_provider(&app, "claude").await;
        assert_eq!(
            ok.status(),
            StatusCode::OK,
            "a configured provider must be accepted"
        );
    }

    /// The session PATCH applies the same up-front provider guard: for a resolvable
    /// session, an unconfigured provider is rejected with 400 before the
    /// rename/auto-reopen sub-commands run, so a bad provider cannot land after an
    /// earlier field already committed. An unknown session still 404s (never a
    /// silent partial apply).
    #[tokio::test]
    async fn session_patch_rejects_unconfigured_provider_up_front() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let app = router(handle);

        // Resolvable session `s1`, unconfigured provider → rejected up front (400),
        // before the title/auto-reopen sub-commands could run.
        let bad = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/sessions/s1")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        "{\"title\":\"renamed\",\"provider\":\"frobnicate\"}",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            bad.status(),
            StatusCode::BAD_REQUEST,
            "an unconfigured provider must be rejected up front, before the rename runs"
        );

        // An unknown session 404s rather than silently applying a partial change.
        let missing = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/sessions/does-not-exist")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{\"provider\":\"frobnicate\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            missing.status(),
            StatusCode::NOT_FOUND,
            "an unknown session must 404, never apply a partial change"
        );
    }

    /// With auth off the gate passes; an unknown session resolves to 404 (the
    /// handler is wired and resolves the worktree before doing any git work).
    #[tokio::test]
    async fn nested_git_unknown_session_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/does-not-exist/git/stage")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"path":"a.txt"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// A nested git route with a KNOWN seeded session id resolves the worktree
    /// from `:id` and gets past routing: the path-validation step rejects it as a
    /// non-changed file (400), proving `:id` was extracted rather than 404-ing on
    /// an unknown session. (The seeded worktree is not a real git repo, so the
    /// changed-file membership check fails — a non-routing outcome, which is the
    /// point.)
    #[tokio::test]
    async fn nested_git_stage_resolves_known_session() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let status = oneshot_status(
            &app,
            "POST",
            "/api/v1/sessions/s1/git/stage",
            Some(r#"{"path":"a.txt"}"#),
        )
        .await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "a known session id must resolve past routing (not a 404)"
        );
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "the seeded worktree is not a git repo, so the changed-file guard rejects it as 400"
        );
    }

    /// An oversized `:id` (longer than `MAX_ID_LEN`) is rejected with 404 by the
    /// `id_within_bound` guard before any engine lookup runs.
    #[tokio::test]
    async fn nested_git_oversized_id_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let huge = "x".repeat(crate::rest_common::MAX_ID_LEN + 1);
        let status = oneshot_status(
            &app,
            "POST",
            &format!("/api/v1/sessions/{huge}/git/stage"),
            Some(r#"{"path":"a.txt"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// With auth off the gate passes; an unknown session resolves to 404 (the
    /// write handler resolves the worktree before touching the filesystem).
    #[tokio::test]
    async fn nested_file_unknown_session_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/does-not-exist/files/write")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"path":"a.txt","content":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// A nested file route with a KNOWN seeded session id resolves the worktree
    /// from `:id` and gets past routing: reading a non-existent file in the seeded
    /// worktree is a 400 (a non-routing outcome), proving `:id` was extracted.
    #[tokio::test]
    async fn nested_file_read_resolves_known_session() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let status = oneshot_status(
            &app,
            "POST",
            "/api/v1/sessions/s1/files/read",
            Some(r#"{"path":"does-not-exist.txt"}"#),
        )
        .await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "a known session id must resolve past routing (not a 404)"
        );
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "reading a missing file in the resolved worktree is a 400 client condition"
        );
    }

    /// `/files/tree` bounds concurrent directory listings via
    /// `tree_list_semaphore`, but UNLIKE the ws_*_semaphore connection caps it
    /// must WAIT for a free permit rather than reject with 503: at capacity 1,
    /// two requests fired concurrently must both eventually succeed, serialized
    /// onto the single permit rather than one being refused outright.
    #[tokio::test]
    async fn tree_list_at_capacity_one_waits_instead_of_rejecting() {
        let tmp = tempfile::tempdir().unwrap();
        // A directory with enough entries that the first request's blocking
        // `read_dir` stays in flight long enough for the second, concurrently
        // fired request to genuinely contend on the capacity-1 semaphore.
        for i in 0..4000 {
            std::fs::write(tmp.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let handle = seeded_engine_handle(tmp.path());
        let app = build_app(
            handle,
            Router::new(),
            RouterParams::plain_http().with_tree_list_max_concurrency(1),
        );

        let req = || {
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/sessions/s1/files/tree")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"dir":""}"#))
                .unwrap()
        };

        let app_a = app.clone();
        let app_b = app.clone();
        let (resp_a, resp_b) = tokio::join!(app_a.oneshot(req()), app_b.oneshot(req()));
        assert_eq!(
            resp_a.unwrap().status(),
            StatusCode::OK,
            "a request beyond the capacity-1 semaphore must wait, not 503"
        );
        assert_eq!(
            resp_b.unwrap().status(),
            StatusCode::OK,
            "a request beyond the capacity-1 semaphore must wait, not 503"
        );
    }

    /// An oversized `:id` (longer than `MAX_ID_LEN`) is rejected with 404 by the
    /// `id_within_bound` guard before any engine lookup runs.
    #[tokio::test]
    async fn nested_file_oversized_id_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let huge = "x".repeat(crate::rest_common::MAX_ID_LEN + 1);
        let status = oneshot_status(
            &app,
            "POST",
            &format!("/api/v1/sessions/{huge}/files/read"),
            Some(r#"{"path":"a.txt"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// Boot a headless engine handle whose store holds one project (`p1`) and one
    /// session (`s1`), so the spine reads return non-empty bodies. The git/worktree
    /// paths need not exist — the spine projection reads in-memory engine state, not
    /// the filesystem.
    fn seeded_engine_handle(tmp: &std::path::Path) -> crate::engine_actor::EngineHandle {
        use dux_core::config::{DuxPaths, ProjectConfig};
        use dux_core::storage::SessionStore;

        let root = tmp.to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        {
            let store = SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .upsert_project(&ProjectConfig {
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
            let now = chrono::Utc::now();
            store
                .upsert_session(&dux_core::model::AgentSession {
                    id: "s1".to_string(),
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
                            worktree_path: root.to_string_lossy().into_owned(),
                        },
                    ),
                })
                .unwrap();
        }
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        handle
    }

    /// `GET /api/v1/workspace` returns the projects, sessions, and sidebar projection
    /// (auth off → the gate passes).
    #[tokio::test]
    async fn spine_route_returns_projects_sessions_and_sidebar() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/workspace")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json.get("projects").is_some(), "spine carries projects");
        assert!(json.get("sessions").is_some(), "spine carries sessions");
        assert!(json.get("sidebar").is_some(), "spine carries sidebar");
        assert_eq!(json["projects"][0]["id"], "p1");
        assert_eq!(json["sessions"][0]["id"], "s1");
    }

    /// `GET /api/v1/sessions/:id` is 200 for a known session and 404 for an unknown
    /// one (auth off → the gate passes).
    #[tokio::test]
    async fn session_route_is_200_for_known_and_404_for_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let known = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/sessions/s1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(known.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(known.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["id"], "s1");

        let unknown = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/sessions/does-not-exist")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    /// Helper: issue a request through the real router and return the status.
    async fn oneshot_status(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<&str>,
    ) -> StatusCode {
        let mut builder = axum::http::Request::builder().method(method).uri(uri);
        let body = match body {
            Some(b) => {
                builder = builder.header("content-type", "application/json");
                axum::body::Body::from(b.to_string())
            }
            None => axum::body::Body::empty(),
        };
        app.clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// The body-keyed project git endpoints (`/api/v1/git/pull-project` and
    /// `/api/v1/git/checkout-default`) were removed in favor of the path-keyed
    /// `/api/v1/projects/:id/{pull,checkout-default}` actions, so they must no
    /// longer reach the git handler — like any unregistered `/api/v1/git/*` path,
    /// they now fall through to the SPA static fallback.
    #[tokio::test]
    async fn removed_project_git_routes_are_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        // A path under /api/v1/git that was never a route hits the SPA fallback.
        // The removed project endpoints must now behave identically.
        let fallback = oneshot_status(
            &app,
            "POST",
            "/api/v1/git/definitely-not-a-route",
            Some("{}"),
        )
        .await;
        for uri in ["/api/v1/git/pull-project", "/api/v1/git/checkout-default"] {
            assert_eq!(
                oneshot_status(&app, "POST", uri, Some(r#"{"project_id":"p1"}"#)).await,
                fallback,
                "{uri} should no longer reach a handler (replaced by /api/v1/projects/:id/...)"
            );
        }

        // Contrast: a surviving git route still reaches its handler (an unknown
        // session resolves there), so it does NOT match the fallback status —
        // proving the equality above is route removal, not a blanket fallthrough
        // of everything under /api/v1/git. The git mutations now live under the
        // session-nested path.
        assert_ne!(
            oneshot_status(&app, "POST", "/api/v1/sessions/nope/git/push", Some("{}")).await,
            fallback,
            "the surviving push route must still reach the git handler"
        );
    }

    /// With auth off, the session action routes resolve an unknown session id to
    /// 404 (they resolve the worktree before dispatching any work).
    #[tokio::test]
    async fn session_actions_unknown_session_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let cases: &[(&str, &str, Option<&str>)] = &[
            ("DELETE", "/api/v1/sessions/nope", None),
            ("PATCH", "/api/v1/sessions/nope", Some(r#"{"title":"x"}"#)),
            ("POST", "/api/v1/sessions/nope/reconnect", Some("{}")),
            // Companion-terminal verbs resolve the session first → 404 when unknown.
            ("POST", "/api/v1/sessions/nope/terminals", None),
            ("DELETE", "/api/v1/sessions/nope/terminals/t1", None),
        ];
        for (method, uri, body) in cases {
            assert_eq!(
                oneshot_status(&app, method, uri, *body).await,
                StatusCode::NOT_FOUND,
                "{method} {uri} must 404 for an unknown session"
            );
        }
    }

    /// With auth off, the project action routes resolve an unknown project id to
    /// 404 (they check existence before dispatching).
    #[tokio::test]
    async fn project_actions_unknown_project_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let cases: &[(&str, &str, Option<&str>)] = &[
            ("DELETE", "/api/v1/projects/nope", None),
            (
                "PATCH",
                "/api/v1/projects/nope",
                Some(r#"{"provider":"claude"}"#),
            ),
            ("POST", "/api/v1/projects/nope/pull", None),
            ("POST", "/api/v1/projects/nope/checkout-default", None),
        ];
        for (method, uri, body) in cases {
            assert_eq!(
                oneshot_status(&app, method, uri, *body).await,
                StatusCode::NOT_FOUND,
                "{method} {uri} must 404 for an unknown project"
            );
        }
    }

    /// Bad input on the create routes is a clean 400: a malformed create body and
    /// an unknown project both reject before any worker spawns.
    #[tokio::test]
    async fn create_routes_reject_bad_input_with_400() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        // Malformed discriminator → 400.
        assert_eq!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/sessions",
                Some(r#"{"kind":"bogus"}"#)
            )
            .await,
            StatusCode::BAD_REQUEST,
        );
        // Unknown project → 400 (wire bails before dispatch).
        assert_eq!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/sessions",
                Some(r#"{"kind":"new","project_id":"nope"}"#)
            )
            .await,
            StatusCode::BAD_REQUEST,
        );
        // Add project with a non-repo path → 400.
        assert_eq!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/projects",
                Some(r#"{"path":"/definitely/not/a/repo"}"#)
            )
            .await,
            StatusCode::BAD_REQUEST,
        );
    }

    /// The session-nested git/file routes reach their handlers: an unknown session
    /// resolves to 404 (auth off so the gate passes). Body-keyed
    /// `/api/v1/git/*` and `/api/v1/file/*` paths do not exist: they fall through
    /// to the SPA fallback, which never returns the handler's 404.
    #[tokio::test]
    async fn nested_git_and_file_routes_reach_handlers() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        assert_eq!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/sessions/nope/git/stage",
                Some(r#"{"path":"a.txt"}"#)
            )
            .await,
            StatusCode::NOT_FOUND,
            "the nested git route must reach the git handler and 404 the unknown session"
        );
        assert_eq!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/sessions/nope/files/read",
                Some(r#"{"path":"a.txt"}"#)
            )
            .await,
            StatusCode::NOT_FOUND,
            "the nested file route must reach the file handler and 404 the unknown session"
        );
        // Body-keyed paths must not reach the handler (no 404 from it).
        assert_ne!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/git/stage",
                Some(r#"{"session_id":"nope","path":"a.txt"}"#)
            )
            .await,
            StatusCode::NOT_FOUND,
            "the old body-keyed /api/v1/git/* path must be gone"
        );
        assert_ne!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/file/read",
                Some(r#"{"session_id":"nope","path":"a.txt"}"#)
            )
            .await,
            StatusCode::NOT_FOUND,
            "the old body-keyed /api/v1/file/* path must be gone"
        );
    }

    /// The literal `/reorder` segment does not collide with `:id` (a reorder with a
    /// full list against the seeded project is accepted — 200 — not routed into the
    /// `:id` handlers).
    #[tokio::test]
    async fn reorder_segment_does_not_collide_with_id() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        // `/reorder` is its own route, not `:id`. The seeded project p1 has exactly
        // session s1, so reordering to [s1] is accepted (200).
        assert_eq!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/sessions/reorder",
                Some(r#"{"project_id":"p1","session_ids":["s1"]}"#)
            )
            .await,
            StatusCode::OK,
        );
    }

    /// The file-drop route's courtesy check. It is NOT the protection (the
    /// socket's own write gate is), so its only job is turning "your file was
    /// saved and then silently not pasted" into a refusal a viewer can act on.
    /// It must therefore match `may_write`'s answer exactly, including the case
    /// that is easy to get backwards: an UNOWNED pty is not denied, because the
    /// first write claims it.
    #[test]
    fn the_file_drop_courtesy_check_matches_the_real_write_gate() {
        let owners = PtySizeOwners::default();
        let pty = "term-1";
        let driver = owners.next_conn_id();
        let viewer = owners.next_conn_id();

        let denied = |conn: u64| matches!(owners.owners.lock().unwrap().map.get(pty), Some(o) if o.conn_id != conn);

        assert!(
            !denied(viewer),
            "an unowned pty must not be denied: the first write claims it, which \
             is exactly what may_write does"
        );

        owners.claim(pty, driver);
        assert!(!denied(driver), "the owner is never denied");
        assert!(
            denied(viewer),
            "a viewer whose device is not driving must be told before a file is \
             written, not after"
        );

        let _ = owners.release(pty, driver);
        assert!(
            !denied(viewer),
            "once the driver disconnects the pty is unowned again"
        );
    }

    /// Two viewers of one PTY, both taking over EXPLICITLY (`claim` is the
    /// take-over spelling of `claim_for_resize`): the most recently claiming
    /// connection owns it, and after the owner drops the surviving connection can
    /// claim the now-unowned pty. Deliberately scoped to explicit take-overs: a
    /// plain resize from the non-owner would be refused, which is the point of
    /// the claim table in `pty_owners.rs`.
    #[test]
    fn pty_size_owner_is_most_recent_takeover_and_releases_on_drop() {
        let owners = PtySizeOwners::default();
        let pty = "session-1";

        // First viewer claims (sends a size) and owns the PTY. The first claim of
        // an unowned PTY is a change.
        let conn_a = owners.next_conn_id();
        assert!(
            owners.claim(pty, conn_a).is_some(),
            "claiming an unowned PTY changes the owner"
        );
        assert!(
            owners.is_owner(pty, conn_a),
            "the sole claimant owns the PTY"
        );

        // Second viewer takes over EXPLICITLY: most-recent take-over wins.
        let conn_b = owners.next_conn_id();
        assert!(
            owners.claim(pty, conn_b).is_some(),
            "a takeover changes the owner"
        );
        assert!(
            owners.is_owner(pty, conn_b),
            "the later claimant owns the PTY"
        );
        assert!(
            !owners.is_owner(pty, conn_a),
            "the earlier claimant no longer owns the PTY"
        );

        // The owner (B) disconnects and releases ownership.
        let _ = owners.release(pty, conn_b);
        assert!(
            !owners.is_owner(pty, conn_b),
            "a released owner no longer owns it"
        );
        // Now A's next claim takes the unowned PTY.
        assert!(
            owners.claim(pty, conn_a).is_some(),
            "claiming after a release changes the owner"
        );
        assert!(owners.is_owner(pty, conn_a));
    }

    /// `claim` reports whether the owner CHANGED so the handler emits `pty.owner`
    /// only on a real handover: a fresh claim and a takeover are changes (returning
    /// `Some(epoch)`); a same-owner re-claim (an owner re-asserting its size) is not
    /// (returning `None`). The epoch increments on each ownership CHANGE and never
    /// on a same-owner re-claim, so the epoch handed to `pty.owner` is monotonic in
    /// true claim order — the property the client's out-of-order dedup relies on.
    #[test]
    fn pty_size_claim_reports_owner_change_with_monotonic_epoch() {
        let owners = PtySizeOwners::default();
        let pty = "session-7";
        let conn_a = owners.next_conn_id();
        let conn_b = owners.next_conn_id();

        let first = owners.claim(pty, conn_a);
        assert_eq!(first, Some(1), "None -> A is the first ownership change");
        assert_eq!(
            owners.claim(pty, conn_a),
            None,
            "A -> A (re-claim) is not a change and assigns no epoch"
        );
        let second = owners.claim(pty, conn_b);
        assert_eq!(
            second,
            Some(2),
            "A -> B (takeover) is a change and increments the epoch"
        );
        assert!(
            second.unwrap() > first.unwrap(),
            "epochs are strictly monotonic across ownership changes"
        );
    }

    /// The epoch is shared across PTYs from one monotonic counter and is bumped by
    /// BOTH the size-frame `claim` path and the first-writer `may_write` path, so a
    /// `pty.owner` from either source orders correctly against the other. Mixing the
    /// two on different ptys still yields strictly increasing epochs.
    #[test]
    fn pty_owner_epoch_is_monotonic_across_claim_and_may_write() {
        let owners = PtySizeOwners::default();
        let conn_a = owners.next_conn_id();
        let conn_b = owners.next_conn_id();

        // A size-frame claim on one pty: epoch 1.
        assert_eq!(owners.claim("pty-a", conn_a), Some(1));
        // A first-writer claim on another pty (the `may_write` path): epoch 2.
        let w = owners.may_write("pty-b", conn_b, None);
        assert_eq!(
            w,
            WriteClaim {
                allowed: true,
                claimed_new: true,
                epoch: Some(2),
            },
            "an unowned-PTY first write claims and takes the next epoch"
        );
        // A takeover back on the first pty: epoch 3 (still strictly increasing).
        assert_eq!(owners.claim("pty-a", conn_b), Some(3));
        // A same-owner re-write on pty-b does not advance the epoch and emits none.
        assert_eq!(
            owners.may_write("pty-b", conn_b, None),
            WriteClaim {
                allowed: true,
                claimed_new: false,
                epoch: None,
            },
            "the owner re-writing claims nothing and carries no epoch"
        );
    }

    /// Owner-only input, exercising the ACTUAL gate the handler applies
    /// (`may_write`, not `is_owner`): with two connections attached to the same PTY,
    /// only the current owner's stdin is forwarded; a non-owner's stdin is dropped.
    /// A read-only secondary viewer can never disrupt the active device's typing,
    /// and a non-owner's denied write never silently claims ownership.
    #[test]
    fn non_owner_stdin_is_dropped() {
        let owners = PtySizeOwners::default();
        let pty = "session-1";

        let conn_a = owners.next_conn_id();
        let conn_b = owners.next_conn_id();
        // Both attach. A claims first (sends a size), then B claims (the most recent
        // foreground device) and takes over.
        owners.claim(pty, conn_a);
        owners.claim(pty, conn_b);

        // The handler forwards a stdin frame only when `may_write` allows it.
        assert_eq!(
            owners.may_write(pty, conn_b, None),
            WriteClaim {
                allowed: true,
                claimed_new: false,
                epoch: None,
            },
            "the owner B's stdin is forwarded without re-claiming"
        );
        assert_eq!(
            owners.may_write(pty, conn_a, None),
            WriteClaim {
                allowed: false,
                claimed_new: false,
                epoch: None,
            },
            "the non-owner A's stdin is dropped and does not claim ownership"
        );
        // The denied write left ownership untouched: B still owns the PTY.
        assert!(
            owners.is_owner(pty, conn_b),
            "a denied non-owner write must not change the owner"
        );
    }

    /// `may_write` resolves the stdin gate atomically and shares `claim`'s
    /// unowned-PTY semantics: the owner is allowed (no re-claim), a non-owner is
    /// denied, and an UNOWNED PTY's first writer is allowed AND becomes the owner so
    /// a solo/out-of-band client whose stdin arrives before any size frame is no
    /// longer silently dropped.
    #[test]
    fn may_write_allows_owner_denies_non_owner_and_claims_unowned() {
        let owners = PtySizeOwners::default();
        let pty = "session-42";
        let conn_a = owners.next_conn_id();
        let conn_b = owners.next_conn_id();

        // Unowned PTY: the first writer is allowed and NEWLY claims ownership,
        // taking the first ownership epoch so its `pty.owner` handover is ordered.
        assert_eq!(
            owners.may_write(pty, conn_a, None),
            WriteClaim {
                allowed: true,
                claimed_new: true,
                epoch: Some(1),
            },
            "an unowned PTY's first writer is allowed and claims ownership"
        );
        assert!(
            owners.is_owner(pty, conn_a),
            "the first writer became the owner"
        );

        // The same owner writing again is allowed without re-claiming (so the
        // caller does not re-emit a `pty.owner` for steady-state typing).
        assert_eq!(
            owners.may_write(pty, conn_a, None),
            WriteClaim {
                allowed: true,
                claimed_new: false,
                epoch: None,
            },
            "the owner writes again without re-claiming"
        );

        // A different connection is denied and does not steal ownership by typing.
        assert_eq!(
            owners.may_write(pty, conn_b, None),
            WriteClaim {
                allowed: false,
                claimed_new: false,
                epoch: None,
            },
            "a non-owner's write is denied"
        );
        assert!(
            owners.is_owner(pty, conn_a),
            "a denied write never wrests ownership from the active owner"
        );
    }

    /// An attached-but-never-claimed connection (a backgrounded observer that sent
    /// neither a size nor a write) owns nothing, so its stdin is dropped: attaching
    /// alone does not grant input. This test covers the pure-observer case only --
    /// `may_write` auto-claims an unowned PTY's first writer, so a connection that
    /// sends a write would become the owner rather than staying an observer.
    #[test]
    fn attach_without_claim_is_a_read_only_observer() {
        let owners = PtySizeOwners::default();
        let pty = "session-9";
        let observer = owners.next_conn_id();
        // The observer attached (got a conn id) but never sent a size.
        assert!(
            !owners.is_owner(pty, observer),
            "an unclaimed PTY grants no one input"
        );
    }

    /// `release` is a no-op when another connection has already claimed the PTY, so
    /// a late-arriving disconnect from a former owner never steals ownership from the
    /// current one.
    #[test]
    fn pty_size_owner_release_does_not_clobber_a_newer_owner() {
        let owners = PtySizeOwners::default();
        let pty = "term-9";

        let conn_a = owners.next_conn_id();
        owners.claim(pty, conn_a);
        let conn_b = owners.next_conn_id();
        owners.claim(pty, conn_b);

        // A disconnects after B took over: releasing A must not drop B's ownership.
        let _ = owners.release(pty, conn_a);
        assert!(
            owners.is_owner(pty, conn_b),
            "B remains the owner after A's stale release"
        );
    }

    /// The spine-change forwarder maps each [`SpineChange`] onto the matching coarse
    /// event on the bus: a sessions change emits `sessions.changed`, a projects
    /// change emits `projects.changed`. This is the "a change emits X on the bus"
    /// contract the frontend subscribes to.
    #[tokio::test]
    async fn spine_forwarder_emits_coarse_events_on_the_bus() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe();
        let (tx, spine_rx) = tokio::sync::broadcast::channel::<SpineChange>(8);
        let _handle = spawn_spine_changed_forwarder(spine_rx, Arc::clone(&bus));

        // A sessions change → `sessions.changed`.
        tx.send(SpineChange::Sessions).unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("event delivered")
            .expect("bus open");
        assert_eq!(
            ev,
            Event::Resource {
                event: "sessions.changed".to_string(),
                id: None,
                rev: None,
                owner: None,
                epoch: None,
                device: None,
            }
        );

        // A projects change → `projects.changed`.
        tx.send(SpineChange::Projects).unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("event delivered")
            .expect("bus open");
        assert_eq!(
            ev,
            Event::Resource {
                event: "projects.changed".to_string(),
                id: None,
                rev: None,
                owner: None,
                epoch: None,
                device: None,
            }
        );
    }

    /// The access middleware logs a request's method, path, and final status when
    /// `access_log` is on and the console is active, and SKIPS `/healthz`. Driven
    /// through the real router (oneshot) so the middleware order (outermost, after
    /// the session layer) is exercised, and captured via the console writer seam.
    #[tokio::test]
    async fn access_log_emits_request_lines_and_skips_healthz() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let (console, sink) = Console::test_capture(false);
        let params = RouterParams::plain_http().with_console(console, true);
        let app = build_app(handle, Router::new(), params);

        // A 200 on an open route is logged.
        let me = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/me")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(me.status(), StatusCode::OK);

        // /healthz is NEVER logged (probe noise).
        let health = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/healthz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        // A 404 on an unknown path is logged with its status. The SPA static
        // fallback serves index.html for unknown non-asset paths, so hit an
        // /api/... path the router has no route for to get a clean 404 — actually
        // the fallback catches everything, so assert on whatever status the
        // fallback returns for a bogus asset path.
        let missing = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/definitely-not-a-real-asset.zzz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let missing_status = missing.status().as_u16();

        let out = sink.contents();
        assert!(
            out.contains("/api/me 200"),
            "the 200 request must be logged: {out}"
        );
        assert!(!out.contains("/healthz"), "/healthz must be skipped: {out}");
        assert!(
            out.contains(&format!(
                "/definitely-not-a-real-asset.zzz {missing_status}"
            )),
            "the fallback request must be logged with its status: {out}"
        );
    }

    /// With `access_log = false` the middleware emits NOTHING even though the
    /// console is active.
    #[tokio::test]
    async fn access_log_toggle_off_emits_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let (console, sink) = Console::test_capture(false);
        // access_log = false.
        let params = RouterParams::plain_http().with_console(console, false);
        let app = build_app(handle, Router::new(), params);

        let _ = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/me")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            sink.contents().is_empty(),
            "access_log = false must emit no access lines: {}",
            sink.contents()
        );
    }

    /// `access_log` is one of the two `[server]` settings a reload applies to a
    /// listener that is already bound: the middleware reads the shared cell the
    /// reload writes, not the value frozen into the router at bind time.
    #[tokio::test]
    async fn a_reloaded_access_log_toggle_takes_effect_on_the_next_request() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let limits = handle.live_limits();
        let (console, sink) = Console::test_capture(false);
        let params = RouterParams::plain_http().with_console(console, false);
        let app = build_app(handle, Router::new(), params);

        let _ = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/me")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(sink.contents().is_empty(), "bound with the log off");

        limits.set_access_log(true);
        let _ = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/me")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let out = sink.contents();
        assert!(
            out.contains("/api/me 200"),
            "turning it on must not need a restart: {out}"
        );
    }

    /// A no-op console (the flip default) emits nothing even with `access_log`
    /// nominally on — the middleware's `console.is_active()` gate short-circuits.
    /// This is the flip zero-stdout regression guard at the middleware layer.
    #[tokio::test]
    async fn access_log_noop_console_emits_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        // The default plain_http params carry a no-op console; force access_log on
        // to prove the console-activity gate (not just the toggle) suppresses it.
        let params = RouterParams {
            console: Console::noop(),
            access_log: true,
            ..RouterParams::plain_http()
        };
        // The router still builds; nothing should panic and nothing is observable
        // (a no-op console drops every line). We assert the request succeeds.
        let app = build_app(handle, Router::new(), params);
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/me")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // --- status_events (on-connect snapshot) unit tests ---

    /// An empty snapshot produces no events.
    #[test]
    fn status_events_empty_snapshot_is_empty() {
        assert!(status_events(&[], "conn").is_empty());
    }

    /// A snapshot with one open entry produces one status event with the correct
    /// key, tone, message, and a serialized `status` envelope.
    #[test]
    fn status_events_single_entry_maps_to_one_event() {
        let snapshot = vec![KeyedWireStatus {
            key: Some("pull".into()),
            tone: "busy".into(),
            message: "Pulling\u{2026}".into(),
            scope: StatusScope::All,
            sticky: false,
        }];
        let events = status_events(&snapshot, "conn");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "status");
        assert_eq!(events[0].key.as_deref(), Some("pull"));
        assert_eq!(events[0].tone, "busy");
        assert_eq!(events[0].message, "Pulling\u{2026}");
        // The serialized shape is `{event,key,tone,message,scope}`.
        let json = serde_json::to_string(&events[0]).unwrap();
        assert_eq!(
            json,
            r#"{"event":"status","key":"pull","tone":"busy","message":"Pulling…","scope":"all","sticky":false}"#
        );
    }

    /// A multi-entry snapshot produces one event per entry, in order.
    #[test]
    fn status_events_multi_entry_produces_n_events() {
        let snapshot = vec![
            KeyedWireStatus {
                key: Some("pull".into()),
                tone: "busy".into(),
                message: "Pulling\u{2026}".into(),
                scope: StatusScope::All,
                sticky: false,
            },
            KeyedWireStatus {
                key: Some("commit".into()),
                tone: "info".into(),
                message: "Changes committed.".into(),
                scope: StatusScope::All,
                sticky: false,
            },
            KeyedWireStatus {
                key: None,
                tone: "warning".into(),
                message: "Worktree dirty.".into(),
                scope: StatusScope::All,
                sticky: false,
            },
        ];
        let events = status_events(&snapshot, "conn");
        assert_eq!(events.len(), 3, "one event per open status entry");
        let keys: Vec<Option<&str>> = events.iter().map(|e| e.key.as_deref()).collect();
        assert_eq!(keys, vec![Some("pull"), Some("commit"), None]);
    }

    /// An entry with an empty message is filtered out (nothing to show).
    #[test]
    fn status_events_empty_message_is_filtered() {
        let snapshot = vec![
            KeyedWireStatus {
                key: Some("op".into()),
                tone: "info".into(),
                message: String::new(),
                scope: StatusScope::All,
                sticky: false,
            },
            KeyedWireStatus {
                key: Some("other".into()),
                tone: "busy".into(),
                message: "Working\u{2026}".into(),
                scope: StatusScope::All,
                sticky: false,
            },
        ];
        let events = status_events(&snapshot, "conn");
        assert_eq!(
            events.len(),
            1,
            "empty-message entries must be filtered out"
        );
        assert_eq!(events[0].key.as_deref(), Some("other"));
    }

    /// A status-clear event serializes to the `{event:"status_cleared", key}` shape.
    #[test]
    fn status_cleared_event_serializes() {
        let ev = WireStatusClearedEvent {
            event: "status_cleared",
            key: Some("pull".into()),
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"status_cleared","key":"pull"}"#
        );
        // A `None` key (anonymous slot) omits the field.
        let anon = WireStatusClearedEvent {
            event: "status_cleared",
            key: None,
        };
        assert_eq!(
            serde_json::to_string(&anon).unwrap(),
            r#"{"event":"status_cleared"}"#
        );
    }

    /// The `connected` handshake serializes to `{event:"connected", id}`.
    #[test]
    fn connected_event_serializes() {
        let ev = WireEvent {
            event: "connected".to_string(),
            id: Some("abc-123".into()),
            rev: None,
            owner: None,
            epoch: None,
            device: None,
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"connected","id":"abc-123"}"#
        );
    }

    /// The PTY-socket `connected` handshake serializes to
    /// `{event, id, gen, owner, owner_epoch}`, a superset of the events-socket
    /// frame carrying the replay generation, the pty's current owner, and the
    /// ownership epoch of that owner snapshot.
    #[test]
    fn pty_connected_frame_serializes_with_generation() {
        let frame = PtyConnectedFrame {
            event: "connected",
            id: "abc-123".into(),
            generation: 7,
            owner: Some("41".into()),
            owner_epoch: 3,
            owner_device: None,
            rows: Some(30),
            cols: Some(100),
            grid_seq: 5,
        };
        assert_eq!(
            serde_json::to_string(&frame).unwrap(),
            r#"{"event":"connected","id":"abc-123","gen":7,"owner":"41","owner_epoch":3,"rows":30,"cols":100,"grid_seq":5}"#
        );
    }

    /// The handshake names the owner's DEVICE alongside the owner id, because a
    /// mere attach hears no `pty.owner` broadcast: without this key a watcher
    /// that simply opened the pane could only title its take-over card with the
    /// generic copy. Unlike `owner`, the key is omitted rather than null when
    /// there is no name to give (absence needs no second meaning here; `owner`
    /// already tells an old server apart), so the no-owner and no-User-Agent
    /// shapes stay byte-identical to what an older client already parses.
    #[test]
    fn pty_connected_frame_names_the_owners_device_and_omits_an_absent_one() {
        let named = PtyConnectedFrame {
            event: "connected",
            id: "abc-123".into(),
            generation: 7,
            owner: Some("41".into()),
            owner_epoch: 3,
            owner_device: Some("Chrome UA".into()),
            rows: Some(30),
            cols: Some(100),
            grid_seq: 5,
        };
        assert_eq!(
            serde_json::to_string(&named).unwrap(),
            r#"{"event":"connected","id":"abc-123","gen":7,"owner":"41","owner_epoch":3,"owner_device":"Chrome UA","rows":30,"cols":100,"grid_seq":5}"#
        );

        let unnamed = PtyConnectedFrame {
            event: "connected",
            id: "abc-123".into(),
            generation: 7,
            owner: Some("41".into()),
            owner_epoch: 3,
            owner_device: None,
            rows: Some(30),
            cols: Some(100),
            grid_seq: 5,
        };
        assert!(
            !serde_json::to_string(&unnamed)
                .unwrap()
                .contains("owner_device"),
            "an owner with no captured User-Agent omits the key entirely"
        );
    }

    /// A WATCHER OF A TERMINAL-UI-DRIVEN PTY learns which device is driving it,
    /// and that device is the dux terminal UI.
    ///
    /// The terminal UI is a participant in the ownership registry while a
    /// background server is serving, and it records a fixed label rather than a
    /// `User-Agent` because it is not a browser. Nothing about the handshake had
    /// to change for that to work, which is exactly the claim under test: the
    /// label travels the same field a browser's does, so a watcher that merely
    /// attached (and therefore hears no `pty.owner` broadcast at all) can still
    /// name the terminal on its take-over card.
    #[test]
    fn the_handshake_names_the_terminal_ui_as_the_driving_device() {
        let owners = PtySizeOwners::default();
        let tui = owners.next_conn_id();
        let epoch = owners
            .claim_for_resize(
                "s1",
                tui,
                false,
                None,
                Some(dux_core::background_serve::TUI_DEVICE_LABEL),
                |_| {},
            )
            .epoch
            .expect("the terminal UI claimed the unowned pty");

        // The watcher's handshake read, verbatim, and the frame built from it.
        let snapshot = owners.current_owner("s1");
        assert_eq!(snapshot.0, Some(tui));
        assert_eq!(snapshot.1, epoch);
        let (owner, owner_epoch, owner_device) = snapshot;
        let frame = PtyConnectedFrame {
            event: "connected",
            id: "watcher-1".into(),
            generation: 1,
            owner: owner.map(|id| id.to_string()),
            owner_epoch,
            owner_device,
            rows: Some(24),
            cols: Some(80),
            grid_seq: owners.grid_seq("s1"),
        };
        let wire = serde_json::to_string(&frame).expect("the frame serializes");
        assert!(
            wire.contains(&format!(
                "\"owner_device\":\"{}\"",
                dux_core::background_serve::TUI_DEVICE_LABEL
            )),
            "a watcher must be told the dux terminal UI is driving: {wire}"
        );
    }

    /// The handshake's grid is what makes a viewer's own divergence knowable at
    /// all, and an unreadable pty spells it as an explicit null rather than
    /// inventing a size. Both keys are always present, for the same reason
    /// `owner` is: a client that finds them ABSENT is talking to a server that
    /// does not answer the question, and must not read that as agreement.
    #[test]
    fn pty_connected_frame_spells_an_unknown_grid_as_explicit_nulls() {
        let frame = PtyConnectedFrame {
            event: "connected",
            id: "abc-123".into(),
            generation: 7,
            owner: None,
            owner_epoch: 0,
            owner_device: None,
            rows: None,
            cols: None,
            grid_seq: 0,
        };
        assert_eq!(
            serde_json::to_string(&frame).unwrap(),
            r#"{"event":"connected","id":"abc-123","gen":7,"owner":null,"owner_epoch":0,"rows":null,"cols":null,"grid_seq":0}"#
        );
    }

    /// THE THREE SHAPES the `expected_owner` field arrives in, and what each
    /// one means to the claim.
    ///
    /// Absent is the ordinary frame and means "no expectation". A parseable id
    /// is the returning owner naming the dead connection it believes it is
    /// succeeding. GARBAGE is the one that matters: a malformed value must be
    /// an EXPLICIT mismatch, because reading it as "no expectation" would turn
    /// a corrupt frame into an unconditional take-over, and that is a silent
    /// steal.
    #[test]
    fn a_resize_frame_reads_an_absent_valid_or_garbage_expected_owner() {
        let absent: PtyResizeFrame =
            serde_json::from_str(r#"{"rows":24,"cols":80}"#).expect("the ordinary frame parses");
        assert_eq!(absent.expected_owner, None);
        assert_eq!(parse_expected_owner(absent.expected_owner.as_deref()), None);

        let valid: PtyResizeFrame =
            serde_json::from_str(r#"{"rows":24,"cols":80,"takeover":true,"expected_owner":"41"}"#)
                .expect("a ghost succession parses");
        assert!(valid.takeover);
        assert_eq!(
            parse_expected_owner(valid.expected_owner.as_deref()),
            Some(41)
        );

        let garbage: PtyResizeFrame = serde_json::from_str(
            r#"{"rows":24,"cols":80,"takeover":true,"expected_owner":"not-a-number"}"#,
        )
        .expect("a malformed value is still a resize frame");
        let parsed = parse_expected_owner(garbage.expected_owner.as_deref());
        assert_eq!(
            parsed,
            Some(UNMATCHABLE_CONN_ID),
            "garbage must be an explicit mismatch, never 'take from anyone'"
        );

        // And the sentinel really is refused by the registry, rather than merely
        // being a value nobody looked at.
        let owners = PtySizeOwners::default();
        let owner = owners.next_conn_id();
        let stranger = owners.next_conn_id();
        owners.claim("p", owner);
        let outcome = owners.claim_for_resize("p", stranger, true, parsed, None, |_| {});
        assert!(
            !outcome.apply && outcome.epoch.is_none(),
            "a take-over whose expected owner could not be parsed must be refused whole"
        );
        assert!(owners.is_owner("p", owner));
    }

    /// The two Text frames a PTY socket accepts are told apart by their FIELD
    /// SETS, and the resize parse is tried first. A beat frame has no
    /// `rows`/`cols`, so it can never be read as a resize; a resize frame's own
    /// fields are unknown to the beat frame, which refuses them.
    ///
    /// The discrimination moved from "beat is required" to `deny_unknown_fields`
    /// so a page that predates the fold of the viewed ping into this message,
    /// and therefore sends a bare `{"viewed":true}`, still parses and still
    /// stamps attention. Requiring `beat` made that frame unparseable and
    /// silently dropped the signal. The window is short (a changed server run
    /// hard reloads the page) but not zero, because the run-identity check
    /// treats an unreachable endpoint as no evidence of a change.
    #[test]
    fn a_beat_frame_and_a_resize_frame_are_never_mistaken_for_each_other() {
        assert!(
            serde_json::from_str::<PtyResizeFrame>(r#"{"beat":7,"viewed":true}"#).is_err(),
            "a beat frame must not satisfy the resize parse that runs first"
        );
        let beat: PtyBeatFrame =
            serde_json::from_str(r#"{"beat":7,"viewed":true}"#).expect("a viewer's beat parses");
        assert_eq!((beat.beat, beat.viewed), (Some(7), true));

        // A WATCHER sends the same frame with `viewed` false, and an older
        // client that omits it entirely must not suppress attention for
        // everybody.
        let watcher: PtyBeatFrame =
            serde_json::from_str(r#"{"beat":8}"#).expect("a watcher's beat parses");
        assert_eq!((watcher.beat, watcher.viewed), (Some(8), false));

        // The pre-fold viewed ping. Nothing to echo, and the attention stamp
        // still lands.
        let legacy: PtyBeatFrame =
            serde_json::from_str(r#"{"viewed":true}"#).expect("a pre-fold viewed ping parses");
        assert_eq!((legacy.beat, legacy.viewed), (None, true));

        assert!(
            serde_json::from_str::<PtyBeatFrame>(r#"{"rows":24,"cols":80}"#).is_err(),
            "a resize frame carries no beat and must not be answered as one"
        );
    }

    /// OWNERSHIP RELEASE IS A DROP GUARD, and that is the whole point of this
    /// test. It used to be a plain statement at the end of the socket handler,
    /// which a panic unwinding through the handler skipped, leaving the pty
    /// recorded to a connection that no longer exists for the life of the
    /// process: a phantom owner nobody can take over from and nobody can type
    /// into. `_conn_guard` beside it had been a Drop guard all along for exactly
    /// this reason.
    #[test]
    fn a_panicking_socket_still_releases_its_pty_ownership() {
        let owners = Arc::new(PtySizeOwners::default());
        let bus = Arc::new(EventBus::new());
        let conn_id = owners.next_conn_id();
        // Claim it the way a granted take-over does, so there is something to
        // release.
        assert!(owners.claim("pty-1", conn_id).is_some());
        assert_eq!(owners.current_owner("pty-1").0, Some(conn_id));

        let panicked = std::panic::catch_unwind({
            let owners = Arc::clone(&owners);
            let bus = Arc::clone(&bus);
            move || {
                let _guard = PtyOwnershipGuard {
                    pty_id: "pty-1".to_string(),
                    conn_id,
                    owners,
                    bus,
                };
                panic!("the socket handler blew up");
            }
        });
        assert!(panicked.is_err(), "the test's own panic must have happened");
        assert_eq!(
            owners.current_owner("pty-1").0,
            None,
            "an unwind must not leave a phantom owner on the pty"
        );
    }

    /// And the console's live-client count comes back down on the same kind of
    /// exit, for the same reason: the increment is one line at the top of the
    /// handler and every path out owes the decrement.
    #[test]
    fn a_panicking_socket_still_counts_its_client_out() {
        let ring = dux_core::activity::ActivityRing::new();
        let console = Console::capture(ring.clone());
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        console.client_connected(ip);
        assert_eq!(ring.connections(), 1);
        let panicked = std::panic::catch_unwind({
            let console = console.clone();
            move || {
                let _guard = ClientCountGuard {
                    console,
                    peer_ip: ip,
                };
                panic!("the socket handler blew up");
            }
        });
        assert!(panicked.is_err(), "the test's own panic must have happened");
        assert_eq!(
            ring.connections(),
            0,
            "an unwind must not leave the count permanently high"
        );
    }

    /// The answer to a beat, which is what lets the browser measure a round trip
    /// and force a plain reconnect on a miss. It echoes the client's own number,
    /// so an answer to a stale beat cannot be counted as an answer to the
    /// current one.
    #[test]
    fn pty_beat_frame_text_answers_with_the_same_number() {
        assert_eq!(pty_beat_frame_text(7), r#"{"event":"beat","n":7}"#);
        assert_eq!(pty_beat_frame_text(0), r#"{"event":"beat","n":0}"#);
    }

    /// THE STRANDED CLIENT, in isolation. A send against a wedged socket holds
    /// the sink lock and never returns, so the handshake and the replay are
    /// bounded. The bound is asserted on the wrapper rather than on a live
    /// socket because the test harness cannot build a `SharedSink` that stalls:
    /// it wraps a real `WebSocket` split half.
    #[tokio::test(start_paused = true)]
    async fn a_send_that_never_completes_is_abandoned_at_the_deadline() {
        let stalled = with_send_deadline(
            std::time::Duration::from_secs(60),
            std::future::pending::<Result<(), ()>>(),
        )
        .await;
        assert!(
            stalled.is_err(),
            "a send that never completes must give up rather than stranding the socket"
        );
    }

    /// And the wrapper is transparent to a send that does complete, in both
    /// directions: it must not turn an ordinary failure into a success or add
    /// latency to a healthy one.
    #[tokio::test(start_paused = true)]
    async fn the_send_deadline_passes_a_completed_send_through_unchanged() {
        let deadline = std::time::Duration::from_secs(60);
        assert_eq!(
            with_send_deadline(deadline, std::future::ready(Ok(()))).await,
            Ok(())
        );
        assert_eq!(
            with_send_deadline(deadline, std::future::ready(Err(()))).await,
            Err(())
        );
    }

    /// THE DEADLINE IS THE CONFIGURED ONE, and a missing or zero value falls
    /// back to the compiled default rather than removing the bound. It stopped
    /// being a hardcoded ten seconds because a send completes when the bytes
    /// ARRIVE: on a cellular link a ten second deadline measured throughput
    /// against a whole scrollback replay and was exceeded deterministically,
    /// leaving a terminal that could never attach at all.
    #[test]
    fn the_opening_send_deadline_is_read_live_and_never_unbounded() {
        let limits = crate::engine_actor::LiveServerLimits::default();
        assert_eq!(
            pty_opening_send_timeout(&limits),
            std::time::Duration::from_secs(
                dux_core::config::DEFAULT_PTY_SEND_TIMEOUT_SECONDS as u64
            ),
            "an unseeded cell must fall back to the default, not to no bound"
        );
        limits.set_pty_send_timeout_seconds(0);
        assert_eq!(
            pty_opening_send_timeout(&limits),
            std::time::Duration::from_secs(
                dux_core::config::DEFAULT_PTY_SEND_TIMEOUT_SECONDS as u64
            ),
            "zero must not mean no bound: that is the one answer this cannot give"
        );
        limits.set_pty_send_timeout_seconds(180);
        assert_eq!(
            pty_opening_send_timeout(&limits),
            std::time::Duration::from_secs(180)
        );
    }

    /// THE BEAT ECHO HAS ITS OWN, SHORT BOUND. It used to share the opening
    /// sends' configurable one, which is a throughput allowance sized for a
    /// whole scrollback replay; spending it on a twenty-five byte echo parks
    /// every other write on the socket behind the sink lock, long past the point
    /// the browser's own answer deadline has dropped the connection.
    #[test]
    fn the_beat_echo_deadline_is_short_and_independent_of_the_replay_bound() {
        let limits = crate::engine_actor::LiveServerLimits::default();
        assert!(
            PTY_BEAT_ECHO_TIMEOUT < pty_opening_send_timeout(&limits),
            "a twenty-five byte echo must not wait as long as a whole replay"
        );
        // And a user who raises the replay allowance does not raise this with it.
        limits.set_pty_send_timeout_seconds(600);
        assert!(PTY_BEAT_ECHO_TIMEOUT < pty_opening_send_timeout(&limits));
        assert!(PTY_BEAT_ECHO_TIMEOUT <= std::time::Duration::from_secs(5));
        assert!(PTY_BEAT_ECHO_TIMEOUT > std::time::Duration::ZERO);
    }

    /// The grid-change event frame, the one pushed to every socket attached to a
    /// pty whose grid moved. Keyed by `event` like the `connected` handshake
    /// beside it, so one client-side parse tells them apart.
    #[test]
    fn pty_size_frame_serializes_as_a_size_event() {
        assert_eq!(
            pty_size_frame_text(30, 100, 4),
            r#"{"event":"size","rows":30,"cols":100,"seq":4}"#
        );
    }

    /// An UNOWNED pty still serializes the `owner` key, as an explicit null. The
    /// client tells "this server does not answer the question" (key absent, fall
    /// back to the foreground guess) from "nobody is driving" (key null, claim
    /// it) by the key's PRESENCE, so skipping it when null would make an unowned
    /// pty indistinguishable from an old server.
    #[test]
    fn pty_connected_frame_spells_an_unowned_pty_as_an_explicit_null() {
        let frame = PtyConnectedFrame {
            event: "connected",
            id: "abc-123".into(),
            generation: 7,
            owner: None,
            owner_epoch: 0,
            owner_device: None,
            rows: Some(24),
            cols: Some(80),
            grid_seq: 0,
        };
        assert_eq!(
            serde_json::to_string(&frame).unwrap(),
            r#"{"event":"connected","id":"abc-123","gen":7,"owner":null,"owner_epoch":0,"rows":24,"cols":80,"grid_seq":0}"#
        );
    }

    /// `next_replay_generation` hands out strictly increasing ids, so every
    /// (re)open's replay is newer than any earlier one and the client's
    /// already-applied guard only ever drops a duplicate/stale blob.
    #[test]
    fn replay_generations_are_strictly_increasing() {
        let a = next_replay_generation();
        let b = next_replay_generation();
        let c = next_replay_generation();
        assert!(a < b, "second generation must exceed the first");
        assert!(b < c, "third generation must exceed the second");
    }

    /// A `pty.owner` handover from the production `pty_owner_event` helper carries
    /// BOTH the claimer's connection id (`owner`) and the ownership `epoch`, and the
    /// wire frame built from it (exactly as the dispatch loop does) serializes both
    /// so the client can compare the owner id and dedup by epoch.
    #[test]
    fn pty_owner_event_carries_owner_and_epoch_and_serializes() {
        let ev = pty_owner_event("session-1", 42, 7, None);
        let Event::Resource {
            event,
            id,
            rev,
            owner,
            epoch,
            device,
        } = ev;
        assert_eq!(event, "pty.owner");
        assert_eq!(id.as_deref(), Some("session-1"));
        assert_eq!(rev, None);
        assert_eq!(owner.as_deref(), Some("42"), "the claimer id is carried");
        assert_eq!(epoch, Some(7), "the ownership epoch is carried");
        assert_eq!(device, None, "no User-Agent means no device");

        let frame = WireEvent {
            event,
            id,
            rev,
            owner,
            epoch,
            device,
        };
        assert_eq!(
            serde_json::to_string(&frame).unwrap(),
            r#"{"event":"pty.owner","id":"session-1","owner":"42","epoch":7}"#,
            "the wire frame includes owner and epoch and omits device when absent (rev is omitted)"
        );
    }

    /// When the claiming connection sent a `User-Agent`, `pty_owner_event` carries it
    /// as `device` and the wire frame serializes it (so the client can name the
    /// other device); `None` omits the field entirely.
    #[test]
    fn pty_owner_event_carries_device_when_present() {
        let ua = "Mozilla/5.0 (Macintosh) Chrome/120.0";
        let ev = pty_owner_event("session-1", 42, 7, Some(ua));
        let Event::Resource { device, .. } = ev.clone();
        assert_eq!(device.as_deref(), Some(ua), "the User-Agent is carried");

        let Event::Resource {
            event,
            id,
            rev,
            owner,
            epoch,
            device,
        } = ev;
        let frame = WireEvent {
            event,
            id,
            rev,
            owner,
            epoch,
            device,
        };
        assert_eq!(
            serde_json::to_string(&frame).unwrap(),
            r#"{"event":"pty.owner","id":"session-1","owner":"42","epoch":7,"device":"Mozilla/5.0 (Macintosh) Chrome/120.0"}"#,
            "the wire frame includes the device string when present"
        );
    }

    // --- status scope filtering ---

    #[test]
    fn scope_delivers_all_reaches_every_connection() {
        assert!(scope_delivers(&StatusScope::All, "A"));
        assert!(scope_delivers(&StatusScope::All, "B"));
    }

    #[test]
    fn scope_delivers_connection_matches_only_its_own_id() {
        let scope = StatusScope::Connection("A".to_string());
        assert!(scope_delivers(&scope, "A"));
        assert!(!scope_delivers(&scope, "B"));
    }

    /// The on-connect snapshot drops another connection's in-progress `Busy`: a
    /// client joining mid-operation must NOT inherit a ghost spinner. An `All`
    /// status in the same snapshot still reaches it.
    #[test]
    fn status_events_filters_other_connections_busy_from_snapshot() {
        let snapshot = vec![
            KeyedWireStatus {
                key: Some("push".into()),
                tone: "busy".into(),
                message: "Pushing\u{2026}".into(),
                scope: StatusScope::Connection("A".into()),
                sticky: false,
            },
            KeyedWireStatus {
                key: Some("commit".into()),
                tone: "info".into(),
                message: "Changes committed.".into(),
                scope: StatusScope::All,
                sticky: false,
            },
        ];
        // Connection B joins: it sees only the `All` status, not A's busy.
        let events = status_events(&snapshot, "B");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key.as_deref(), Some("commit"));
        // Connection A sees both (its own busy + the broadcast).
        assert_eq!(status_events(&snapshot, "A").len(), 2);
    }

    /// An older peer's / the TUI's `WireStatus` JSON with no `scope` field
    /// deserializes to `All`, so it still reaches every connection.
    #[test]
    fn wire_status_without_scope_defaults_to_all() {
        let json = r#"{"tone":"info","message":"Saved."}"#;
        let ws: dux_core::wire::WireStatus = serde_json::from_str(json).unwrap();
        assert_eq!(ws.scope, StatusScope::All);
        assert!(scope_delivers(&ws.scope, "any-connection"));
    }

    // --- bootstrap route ---

    /// With auth off the gate passes; `GET /api/v1/bootstrap` returns 200 with a
    /// JSON object carrying EXACTLY the build-/config-static fields the frontend
    /// expects (the 11 fields moved off the per-tick ViewModel).
    #[tokio::test]
    async fn bootstrap_route_returns_expected_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/bootstrap")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let obj = json.as_object().expect("bootstrap must be a JSON object");
        for field in [
            "available_providers",
            "macros",
            "welcome_tips",
            "dux_version",
            "randomize_agent_names_by_default",
            "gh_available",
            "pr_banner_position",
            "agent_scrollback_lines",
            "show_changes_pane",
            "global_env",
            // The first-load screens: the welcome copy and the website link ride
            // here unconditionally (config-static, and the app menu can open the
            // screen on demand), the two suppression flags feed the Preferences
            // rows, and `pending_first_load` is the per-launch decision the
            // handler injects (present as an explicit null when no screen is due).
            "welcome_screen",
            "website_url",
            "pending_first_load",
            "disable_automated_welcome_screen",
            "disable_release_notes",
        ] {
            assert!(
                obj.contains_key(field),
                "bootstrap JSON must carry `{field}`: {json}"
            );
        }
        // The volatile spine must NOT leak into bootstrap.
        assert!(!obj.contains_key("projects"), "bootstrap is config-static");
        assert!(!obj.contains_key("sessions"), "bootstrap is config-static");
    }

    // --- config.changed forwarder ---

    /// The forwarder turns one engine reload signal into a coarse `config.changed`
    /// event on the bus (no id/rev). Deterministic: drives the broadcast directly.
    #[tokio::test]
    async fn config_changed_forwarder_emits_on_reload_signal() {
        let (tx, rx) = tokio::sync::broadcast::channel::<()>(8);
        let bus = Arc::new(EventBus::new());
        let mut bus_rx = bus.subscribe();
        let _h = spawn_config_changed_forwarder(rx, Arc::clone(&bus));

        tx.send(()).unwrap();

        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), bus_rx.recv())
            .await
            .expect("config.changed should be emitted within the timeout")
            .expect("bus recv");
        assert_eq!(ev, config_changed_event());
    }

    /// End-to-end: a REAL config reload through the engine actor fires the reload
    /// broadcast, which the forwarder turns into `config.changed` on the bus. This
    /// is the chain a `config`-subscribed client relies on to refetch bootstrap.
    #[tokio::test]
    async fn real_config_reload_emits_config_changed_on_the_bus() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let bus = Arc::new(EventBus::new());
        let mut bus_rx = bus.subscribe();
        let _h =
            spawn_config_changed_forwarder(handle.subscribe_config_reloads(), Arc::clone(&bus));

        // Drive a real reload (read-only re-load of config.toml; defaults when
        // absent). The actor completes it on a later tick and fires the reload
        // broadcast, which the forwarder converts to `config.changed`.
        handle
            .apply_wire(dux_core::wire::WireCommand::ReloadConfig {})
            .await
            .expect("reload command");

        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), bus_rx.recv())
            .await
            .expect("a config reload must emit config.changed")
            .expect("bus recv");
        assert_eq!(ev, config_changed_event());
    }

    /// End-to-end regression: saving macros through the engine actor (the
    /// `PUT /api/v1/macros` path) must ALSO emit `config.changed` on the bus, just
    /// like a reload does. The macro is written to disk and adopted in memory, but
    /// without this signal a `config`-subscribed client never refetches bootstrap,
    /// so the macro dialog reseeds from a stale list and the just-saved macro
    /// appears to vanish. The eager-save config mutations (`UpdateMacros`,
    /// `PersistGlobalEnv`, `SetChangesPaneVisible`) share this chain.
    #[tokio::test]
    async fn macro_save_emits_config_changed_on_the_bus() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let bus = Arc::new(EventBus::new());
        let mut bus_rx = bus.subscribe();
        let _h =
            spawn_config_changed_forwarder(handle.subscribe_config_reloads(), Arc::clone(&bus));

        // Save one macro wholesale, exactly as the REST verb does.
        handle
            .apply_wire(dux_core::wire::WireCommand::UpdateMacros {
                entries: vec![dux_core::wire::WireMacroEntry {
                    name: "greet".to_string(),
                    text: "hi".to_string(),
                    surface: "agent".to_string(),
                }],
            })
            .await
            .expect("update macros command");

        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), bus_rx.recv())
            .await
            .expect("a macro save must emit config.changed")
            .expect("bus recv");
        assert_eq!(ev, config_changed_event());
    }

    /// The same chain for the workspace-wide env editor (`PUT /api/v1/global-env`):
    /// it shares the macro path's gap, so persisting the env map must likewise emit
    /// `config.changed` so clients refetch bootstrap.
    #[tokio::test]
    async fn global_env_save_emits_config_changed_on_the_bus() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let bus = Arc::new(EventBus::new());
        let mut bus_rx = bus.subscribe();
        let _h =
            spawn_config_changed_forwarder(handle.subscribe_config_reloads(), Arc::clone(&bus));

        let mut env = std::collections::BTreeMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        handle
            .apply_wire(dux_core::wire::WireCommand::PersistGlobalEnv { env })
            .await
            .expect("persist global env command");

        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), bus_rx.recv())
            .await
            .expect("a global-env save must emit config.changed")
            .expect("bus recv");
        assert_eq!(ev, config_changed_event());
    }

    // ── Host guard serve-level tests ───────────────────────────────────────

    /// When the host guard is active (bound_ips set), an unknown Host is rejected
    /// with 403 before reaching any handler; `localhost` passes through.
    #[tokio::test]
    async fn host_guard_rejects_unknown_host_and_allows_localhost() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(
            handle,
            Router::new(),
            RouterParams::plain_http().with_host_allowlist(
                vec!["127.0.0.1".parse().unwrap()],
                vec![],
                false,
            ),
        );

        // An unknown hostname gets 403 (DNS-rebinding defense).
        let bad = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/healthz")
                    .header("Host", "evil.example.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            bad.status(),
            StatusCode::FORBIDDEN,
            "unknown Host must be rejected"
        );

        // `localhost` is always allowed (rule 1).
        let good = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/healthz")
                    .header("Host", "localhost")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(good.status(), StatusCode::OK, "localhost must be allowed");
    }

    /// Rule 5 end to end: with the Tailscale mode on, a request carrying a `100.x`
    /// Host is served even though only loopback is bound. That combination is the
    /// normal state on `auto` (the interface is away, or it just came back and the
    /// router was built long before), and it must not be a 403. An unknown NAME is
    /// still refused, which is the property the guard exists for.
    #[tokio::test]
    async fn host_guard_follows_a_live_tailscale_mode_change_on_the_same_router() {
        // The router is built once per serve and the mode can change under it, so
        // the whole stack (not just the allowlist) has to answer differently on
        // the same app instance.
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let literals = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let app = build_app(
            handle,
            Router::new(),
            RouterParams::plain_http()
                .with_host_allowlist(vec!["127.0.0.1".parse().unwrap()], vec![], false)
                .with_live_tailscale_host_literals(Arc::clone(&literals)),
        );
        let tailnet_probe = || {
            axum::http::Request::builder()
                .method("GET")
                .uri("/healthz")
                .header("Host", "100.101.102.103:8080")
                .body(axum::body::Body::empty())
                .unwrap()
        };

        let refused = app.clone().oneshot(tailnet_probe()).await.unwrap();
        assert_eq!(
            refused.status(),
            StatusCode::FORBIDDEN,
            "the mode is no, so a tailnet Host is refused"
        );

        literals.store(true, std::sync::atomic::Ordering::SeqCst);
        let served = app.oneshot(tailnet_probe()).await.unwrap();
        assert_eq!(
            served.status(),
            StatusCode::OK,
            "the same router must serve it once the mode wants Tailscale"
        );
    }

    #[tokio::test]
    async fn host_guard_serves_a_tailnet_host_while_only_loopback_is_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(
            handle,
            Router::new(),
            RouterParams::plain_http().with_host_allowlist(
                vec!["127.0.0.1".parse().unwrap()],
                vec![],
                true,
            ),
        );

        let tailnet = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/healthz")
                    .header("Host", "100.101.102.103:8080")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            tailnet.status(),
            StatusCode::OK,
            "a tailnet IP literal must be served even with the leg unbound"
        );

        let name = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/healthz")
                    .header("Host", "evil.example.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            name.status(),
            StatusCode::FORBIDDEN,
            "widening literals must not widen names"
        );
    }

    // ── REST same-origin serve-level tests ────────────────────────────────

    /// A cross-origin POST to a mutation route is rejected with 403 (cross-site
    /// request forgery defense). The Origin authority does not match Host.
    #[tokio::test]
    async fn cross_origin_post_mutation_is_403() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/s1/git/stage")
                    .header("Host", "localhost")
                    .header("Origin", "http://evil.example.com")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"path":"a.txt"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "cross-origin POST must be rejected"
        );
    }

    /// A POST with `Origin: null` (sandboxed iframe / data: document) is always
    /// rejected with 403, regardless of bind address. `null` has no parseable
    /// authority, so `same_origin_allowed` treats it as a mismatch -- it must
    /// NOT fall through to the no-Origin allow path.
    #[tokio::test]
    async fn post_with_null_origin_is_403() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        // No host guard needed -- the Origin check fires independently.
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/s1/git/stage")
                    .header("Host", "localhost")
                    .header("Origin", "null")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"path":"a.txt"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Origin: null must be treated as cross-origin and rejected"
        );
    }

    /// A POST with NO `Origin` header reaches the handler (non-browser client).
    /// `same_origin_allowed` allows missing-Origin explicitly -- a curl/CLI
    /// client is trusted to not be a hijacked browser tab.
    #[tokio::test]
    async fn post_with_no_origin_reaches_handler() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let app = build_app(handle, Router::new(), RouterParams::plain_http());

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/does-not-exist/git/stage")
                    .header("Host", "localhost")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"path":"a.txt"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Reaches the git handler -- unknown session -> 404, NOT 403.
        assert_ne!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "a POST without Origin must reach the handler"
        );
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "the git handler returns 404 for an unknown session"
        );
    }

    // ── WebSocket cross-origin serve-level tests ───────────────────────────
    //
    // The WS upgrade handlers use `WebSocketUpgrade` as a function argument,
    // which requires a real `hyper::upgrade::OnUpgrade` extension that axum
    // inserts only for genuine TCP connections. `oneshot` in tower does not
    // set this extension, so `WebSocketUpgrade` extraction fails (426) before
    // the handler body -- and therefore before the origin check -- can run.
    // These tests must use a real bound server via `boot_plain_test_server`.

    /// A cross-origin WS upgrade to ALL THREE WS endpoints is rejected with 403
    /// (cross-site WebSocket hijacking defense). Each handler checks
    /// `same_origin_allowed` before the upgrade completes: a mismatched `Origin`
    /// never gets to subscribe to PTY output or events. The origin check fires
    /// before any session/terminal lookup, so seeding data is unnecessary.
    ///
    /// Uses raw TCP so the WS upgrade request can carry an arbitrary `Origin`
    /// header without client-side WS-library pre-flight validation interfering
    /// (tungstenite's `IntoClientRequest` rejects unrecognized fields; reqwest
    /// strips hop-by-hop headers). A raw write exercises the same server code path
    /// as a real browser WS upgrade: the server sees an HTTP/1.1 GET with WS
    /// headers and runs the handler before any upgrade handshake.
    #[tokio::test]
    async fn cross_origin_ws_upgrades_rejected_on_all_three_endpoints() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // A real bound server so the `WebSocketUpgrade` extractor can find the
        // hyper upgrade extension (which `oneshot` does not provide).
        let (_tmp, addr) = crate::test_support::boot_plain_test_server().await;

        let paths = [
            "/ws/events",
            "/ws/sessions/s1/pty",
            "/ws/sessions/s1/terminals/t1/pty",
        ];

        for path in &paths {
            // Send a raw HTTP/1.1 WS upgrade request with a cross-origin Origin.
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let request = format!(
                "GET {path} HTTP/1.1\r\n\
                 Host: {addr}\r\n\
                 Origin: http://evil.example.com\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 \r\n"
            );
            stream.write_all(request.as_bytes()).await.unwrap();

            // Read the HTTP response (status line is first).
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            let response = String::from_utf8_lossy(&buf[..n]);
            // Status line is "HTTP/1.1 403 Forbidden\r\n..." or similar.
            assert!(
                response.starts_with("HTTP/1.1 403"),
                "{path} must reject cross-origin WS upgrade with 403; got: {response}"
            );
        }
    }

    // --- subscribe catch-up ---

    /// Subscribing to `session:<id>:changes` when the ChangesService cache is
    /// warm (`peek_rev` returns `Some(N)`) causes `apply_events_frame` to return
    /// the newly-inserted fine topic, and `catchup_frames` (the REAL production
    /// helper, not a manual WireEvent build) produces a frame carrying rev N.
    /// This exercises the topic-parse + rev-read integration path.
    #[tokio::test]
    async fn subscribe_emits_catchup_with_current_rev() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let bus = Arc::new(EventBus::new());
        // Seed the cache so peek_rev("s1") == Some(42).
        let changes = ChangesService::new(handle.clone(), Arc::clone(&bus));
        changes.seed_rev_for_test("s1", 42);
        assert_eq!(
            changes.peek_rev("s1"),
            Some(42),
            "pre-condition: cache is warm"
        );

        let mut subscribed = std::collections::HashSet::new();
        let sub_frame = EventsClientFrame {
            subscribe: vec![event_bus::changes_topic("s1")],
            unsubscribe: vec![],
        };
        let new_fine = apply_events_frame(&sub_frame, &mut subscribed, &handle, &bus)
            .await
            .fine;

        // The topic was newly inserted and returned for catch-up.
        assert_eq!(
            new_fine,
            vec![event_bus::changes_topic("s1")],
            "the newly-inserted fine topic must be returned"
        );

        // Call the REAL production dispatch helper — not a hand-built WireEvent.
        let frames = catchup_frames(&new_fine, &changes);
        assert_eq!(
            frames.len(),
            1,
            "one catch-up frame per newly-subscribed session"
        );
        assert_eq!(
            serde_json::to_string(&frames[0]).unwrap(),
            r#"{"event":"session.changes","id":"s1","rev":42}"#,
            "warm-cache catch-up must carry the current rev"
        );
    }

    /// A connection that lagged the event bus is caught up with the CURRENT
    /// workspace document, alongside the coarse nudges it already got. Lag is
    /// the one path where a client can miss a push outright, so the recovery
    /// has to carry the value, not just another "go and ask".
    #[tokio::test]
    async fn a_lagged_connection_is_caught_up_with_the_workspace_document() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let bus = Arc::new(EventBus::new());
        let changes = ChangesService::new(handle.clone(), Arc::clone(&bus));
        let doc = WorkspaceDoc {
            rev: 7,
            json: r#"{"rev":7,"sessions":[]}"#.into(),
        };

        let subscribed: std::collections::HashSet<String> =
            ["sessions".to_string()].into_iter().collect();
        let texts = lagged_catchup_texts(&subscribed, &changes, Some(&doc));
        assert!(
            texts.iter().any(|t| t == r#"{"event":"sessions.changed"}"#),
            "the coarse nudge stays, for a client that does not read the document: {texts:?}"
        );
        assert!(
            texts.iter().any(
                |t| t == r#"{"event":"workspace","rev":7,"workspace":{"rev":7,"sessions":[]}}"#
            ),
            "and the document itself rides along: {texts:?}"
        );

        // A connection holding neither coarse topic is caught up with neither.
        let other: std::collections::HashSet<String> = ["config".to_string()].into_iter().collect();
        let texts = lagged_catchup_texts(&other, &changes, Some(&doc));
        assert_eq!(
            texts,
            vec![r#"{"event":"config.changed"}"#.to_string()],
            "a lagged connection is only caught up on what it holds"
        );

        // Before the engine has published anything there is nothing truthful to
        // send, and the client's boot fetch covers that window.
        let texts = lagged_catchup_texts(&subscribed, &changes, None);
        assert_eq!(
            texts,
            vec![r#"{"event":"sessions.changed"}"#.to_string()],
            "no document published yet means no document frame"
        );
    }

    /// Subscribing to a coarse workspace topic asks for a replay of the current
    /// document; re-subscribing to one already held does not. A connection that
    /// already holds the topic is already being pushed every change to it, so a
    /// repeat subscribe frame must not cost a whole document.
    #[tokio::test]
    async fn only_a_newly_held_coarse_topic_asks_for_a_workspace_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let bus = Arc::new(EventBus::new());
        let mut subscribed = std::collections::HashSet::new();

        let frame = EventsClientFrame {
            subscribe: vec!["sessions".to_string()],
            unsubscribe: vec![],
        };
        assert!(
            apply_events_frame(&frame, &mut subscribed, &handle, &bus)
                .await
                .workspace,
            "a newly-registered coarse topic must ask for the replay"
        );
        assert!(
            !apply_events_frame(&frame, &mut subscribed, &handle, &bus)
                .await
                .workspace,
            "a repeat subscribe to a held topic must not"
        );

        // `config` is a coarse topic too, but it is not the workspace's.
        let mut subscribed = std::collections::HashSet::new();
        let frame = EventsClientFrame {
            subscribe: vec!["config".to_string()],
            unsubscribe: vec![],
        };
        assert!(
            !apply_events_frame(&frame, &mut subscribed, &handle, &bus)
                .await
                .workspace,
            "the config topic must not drag the workspace document along"
        );
    }

    /// Subscribing when the cache is cold (`peek_rev` returns `None`) still
    /// causes `apply_events_frame` to return the fine topic, and `catchup_frames`
    /// produces a frame that omits `rev` (serialised as absent). The client treats
    /// an absent `rev` as a force-refetch, so the changes pane converges even for
    /// a session whose cache has not been populated yet.
    #[tokio::test]
    async fn subscribe_cold_cache_emits_revless_catchup() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let bus = Arc::new(EventBus::new());
        // No seed: the cache is empty, so peek_rev returns None.
        let changes = ChangesService::new(handle.clone(), Arc::clone(&bus));
        assert_eq!(
            changes.peek_rev("s1"),
            None,
            "pre-condition: cache must be cold"
        );

        let mut subscribed = std::collections::HashSet::new();
        let sub_frame = EventsClientFrame {
            subscribe: vec![event_bus::changes_topic("s1")],
            unsubscribe: vec![],
        };
        let new_fine = apply_events_frame(&sub_frame, &mut subscribed, &handle, &bus)
            .await
            .fine;

        assert_eq!(
            new_fine,
            vec![event_bus::changes_topic("s1")],
            "the newly-inserted fine topic must be returned even for a cold cache"
        );

        // Call the REAL production dispatch helper.
        let frames = catchup_frames(&new_fine, &changes);
        assert_eq!(
            frames.len(),
            1,
            "one catch-up frame per newly-subscribed session"
        );
        assert_eq!(
            serde_json::to_string(&frames[0]).unwrap(),
            r#"{"event":"session.changes","id":"s1"}"#,
            "cold-cache catch-up must omit rev so the client force-refetches"
        );
    }

    /// Build an engine handle with TWO sessions (s1 and s2) for multi-topic tests.
    fn seeded_engine_handle_two_sessions(
        tmp: &std::path::Path,
    ) -> crate::engine_actor::EngineHandle {
        use dux_core::config::{DuxPaths, ProjectConfig};
        use dux_core::storage::SessionStore;

        let root = tmp.to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        {
            let store = SessionStore::open(&paths.sessions_db_path).unwrap();
            store
                .upsert_project(&ProjectConfig {
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
            let now = chrono::Utc::now();
            for sid in ["s1", "s2"] {
                store
                    .upsert_session(&dux_core::model::AgentSession {
                        id: sid.to_string(),
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
                                branch_name: format!("feat-{sid}"),
                                initial_branch: format!("feat-{sid}"),
                                branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                                worktree_path: root.to_string_lossy().into_owned(),
                            },
                        ),
                    })
                    .unwrap();
            }
        }
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        handle
    }

    /// Subscribing to TWO fine topics in one `apply_events_frame` call causes
    /// `catchup_frames` to return TWO frames -- one per session, each carrying the
    /// correct id and rev from the seeded cache.
    #[tokio::test]
    async fn subscribe_two_topics_emits_two_catchup_frames() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle_two_sessions(tmp.path());
        let bus = Arc::new(EventBus::new());
        let changes = ChangesService::new(handle.clone(), Arc::clone(&bus));
        // Seed two sessions with distinct revs.
        changes.seed_rev_for_test("s1", 10);
        changes.seed_rev_for_test("s2", 20);
        assert_eq!(changes.peek_rev("s1"), Some(10), "s1 cache must be seeded");
        assert_eq!(changes.peek_rev("s2"), Some(20), "s2 cache must be seeded");

        let mut subscribed = std::collections::HashSet::new();
        let sub_frame = EventsClientFrame {
            subscribe: vec![
                event_bus::changes_topic("s1"),
                event_bus::changes_topic("s2"),
            ],
            unsubscribe: vec![],
        };
        let new_fine = apply_events_frame(&sub_frame, &mut subscribed, &handle, &bus)
            .await
            .fine;

        assert_eq!(
            new_fine.len(),
            2,
            "both fine topics must be returned for catch-up"
        );

        let frames = catchup_frames(&new_fine, &changes);
        assert_eq!(frames.len(), 2, "one catch-up frame per subscribed session");

        // Sort by session id for a deterministic assertion.
        let mut jsons: Vec<String> = frames
            .iter()
            .map(|f| serde_json::to_string(f).unwrap())
            .collect();
        jsons.sort();
        assert_eq!(
            jsons[0], r#"{"event":"session.changes","id":"s1","rev":10}"#,
            "s1 catch-up frame must carry rev 10"
        );
        assert_eq!(
            jsons[1], r#"{"event":"session.changes","id":"s2","rev":20}"#,
            "s2 catch-up frame must carry rev 20"
        );
    }

    #[test]
    fn one_class_saturated_does_not_block_another() {
        // Independence: exhausting one connection class must not starve another.
        // Terminal cap is 0 (refuse all) while events cap is 1.
        let events = Arc::new(tokio::sync::Semaphore::new(1));
        let terminal = Arc::new(tokio::sync::Semaphore::new(0));
        let ip = std::net::IpAddr::from([127, 0, 0, 1]);

        // The saturated terminal class refuses (None).
        assert!(
            acquire_ws_permit(&terminal, ip, "/ws/.../terminals", "max_websocket_terminal")
                .is_none(),
            "a zero-cap terminal class must refuse"
        );
        // The independent events class still hands out a permit.
        assert!(
            acquire_ws_permit(&events, ip, "/ws/events", "max_websocket_events").is_some(),
            "a non-zero events class must still acquire while terminal is saturated"
        );
    }

    #[test]
    fn permit_releases_on_drop() {
        // Lifecycle: acquiring drops the available count; dropping recovers it.
        let sem = Arc::new(tokio::sync::Semaphore::new(2));
        let ip = std::net::IpAddr::from([127, 0, 0, 1]);
        assert_eq!(sem.available_permits(), 2);

        let permit = acquire_ws_permit(&sem, ip, "/ws/.../pty", "max_websocket_agent")
            .expect("first permit");
        assert_eq!(
            sem.available_permits(),
            1,
            "available drops while a permit is held"
        );

        drop(permit);
        assert_eq!(
            sem.available_permits(),
            2,
            "available recovers when the permit drops"
        );
    }
}
