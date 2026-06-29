//! axum router + the WebSocket handlers (`/ws/events` and the per-PTY sockets)
//! bridging the browser to the engine actor. All data reads and actions go over
//! REST (`/api/v1/*`); the sockets carry only change events + status (events) and
//! terminal byte streams (PTY).
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
//! - GATED: every `/api/v1/*` route and every WS upgrade (`/ws/events` and the
//!   per-PTY sockets). When auth is on, the gate middleware rejects with `401`
//!   BEFORE the WS upgrade, so the browser sees a clean HTTP response rather than
//!   a socket that opens and immediately closes.
//!
//! The Origin check on every WS upgrade runs REGARDLESS of auth (cross-site
//! WebSocket hijacking defense): a browser attaches the page's `Origin`, and we
//! only allow same-host origins. Non-browser clients (no `Origin`) are allowed —
//! documented tradeoff: a CLI/test client is trusted to not be a hijacked browser
//! tab.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{SinkExt, StreamExt};
use tower_sessions::cookie::SameSite;
use tower_sessions::cookie::time::Duration as CookieDuration;
use tower_sessions::{Expiry, Session, SessionManagerLayer};

use dux_core::statusline::{KeyedWireStatus, StatusScope};

use crate::auth::{self, RateLimiter, SharedAuth};
use crate::changes::ChangesService;
use crate::console::Console;
use crate::engine_actor::{EngineHandle, SpineChange};
use crate::event_bus::{self, Event, EventBus};
use crate::tls::SweepableMemoryStore;

#[derive(Clone)]
pub struct AppState {
    pub engine: EngineHandle,
    /// Parsed credentials + gate flag, shared so a config reload can rebuild it
    /// (see `engine_actor`). Read briefly by the login/me handlers and the gate.
    pub auth: SharedAuth,
    /// Per-IP login backoff. Shared (cheap `Arc` clone) so all login requests
    /// hit the same counters.
    pub rate_limiter: RateLimiter,
    /// How often a live, authenticated WebSocket re-verifies that its user
    /// still exists (see [`handle_events_socket`]/[`handle_pty_socket`]). The HTTP
    /// gate cannot revoke an ALREADY-upgraded socket, so this periodic recheck
    /// closes that gap. A constructor-injectable field so the revocation e2e can
    /// drive a short window instead of waiting the production cadence.
    pub ws_recheck_period: std::time::Duration,
    /// The `dux server` terminal console. A real (stdout) console on the CLI
    /// serve paths; a [`Console::noop`] for the TUI flip (which owns the
    /// terminal) and every test that does not assert console output. WS/auth
    /// handlers emit life events through it; the access middleware reads it too.
    pub console: Console,
    /// Whether the per-request access log is enabled (`[server] access_log`).
    /// The access middleware checks this AND `console.is_active()` before
    /// emitting, so the flip and disabled-console paths never log.
    pub access_log: bool,
    /// Caps concurrent WebSocket connections (`[server] max_websocket_connections`).
    /// SHARED across EVERY WebSocket family — `/ws/events` (`ws_events_upgrade`) and
    /// the per-PTY sockets (agent/terminal upgrades): each takes a permit before
    /// upgrading and holds it for the socket's lifetime, so the cap bounds the
    /// COMBINED live socket count, not each endpoint separately. When none are free
    /// the upgrade is refused with HTTP 503. A cheap `Arc` clone so every request
    /// hits the same permit pool.
    pub ws_semaphore: Arc<tokio::sync::Semaphore>,
    /// The web-layer event bus: resource-change signals (`/ws/events`) plus the
    /// per-topic interest refcount that drives the changed-files poller.
    pub event_bus: Arc<EventBus>,
    /// The per-session changed-files cache + single-flight compute + poller behind
    /// `GET /api/v1/sessions/:id/changes` and the `session.changes` event. The git
    /// mutation handlers call `state.changes.invalidate(id)` after a successful
    /// stage/unstage/discard/commit/write so the pane refreshes immediately.
    pub changes: Arc<ChangesService>,
    /// `Idempotency-Key -> created resource id` cache (TTL-bounded) so a retried
    /// `POST /api/v1/sessions` or `/projects` after a lost response returns the
    /// same resource instead of creating a duplicate worktree/project.
    pub idempotency: Arc<crate::rest_common::IdempotencyCache>,
    /// Per-PTY sizing ownership so two viewers of one PTY don't thrash its size.
    /// The most recently attached per-PTY socket owns sizing; a non-owner's resize
    /// frame is ignored (see [`PtySizeOwners`]).
    pty_size_owners: Arc<PtySizeOwners>,
}

/// Maximum size of a single inbound WebSocket message (text or binary). This
/// bounds ONE frame — down from axum's untuned 64 MiB default — so a client
/// cannot push an arbitrarily large message; it is NOT a total-memory cap (the
/// theoretical worst case is `REQ_CHANNEL_CAPACITY` queued frames of this size).
/// In practice in-flight memory stays far below that product: the engine drains
/// the whole request channel every tick, and link bandwidth caps how many large
/// frames can even arrive per tick. 16 MiB is far above any realistic terminal
/// paste, so legitimate input is never truncated.
const MAX_WS_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Build the router with the login gate OFF (no `[auth]` users). Kept as the
/// zero-argument entry the existing test harnesses and any no-auth caller use;
/// it delegates to [`router_with_auth`] with an empty, disabled [`AuthState`].
pub fn router(engine: EngineHandle) -> Router {
    router_with_auth(engine, auth::shared_auth(&[], false))
}

/// Build the axum router with an explicit shared auth snapshot, plain-HTTP
/// defaults (no Secure cookie — see [`RouterParams`]). The session store is
/// discarded here; callers that need to sweep expired sessions (the production
/// serve paths) use [`build_app`] instead to keep a handle on the store.
///
/// `auth` carries the parsed credentials and the gate flag; when it reports the
/// gate disabled, the gate middleware passes everything through (today's UX). The
/// session layer is always installed (it is inert when no session is created), so
/// turning auth on via a config reload needs no router rebuild.
pub fn router_with_auth(engine: EngineHandle, auth: SharedAuth) -> Router {
    build_app(engine, auth, Router::new(), RouterParams::plain_http()).0
}

/// Production cadence for the live `/ws` user-existence recheck. Short enough
/// that a revoked user's open socket dies within a few seconds, long enough that
/// the per-socket `Vec` scan under a brief read lock is negligible overhead.
const WS_RECHECK_PERIOD: std::time::Duration = std::time::Duration::from_secs(4);

/// Knobs that differ between the plain-HTTP serve paths and the TLS (ACME) path.
#[derive(Clone)]
pub struct RouterParams {
    /// `/ws` user-existence recheck cadence (injectable so the revocation e2e can
    /// drive a short window).
    pub ws_recheck_period: std::time::Duration,
    /// Whether the session cookie carries the `Secure` attribute. TRUE only when
    /// dux itself terminates TLS (the ACME path); FALSE for plain HTTP, where a
    /// Secure cookie would never be sent over the loopback/proxy deployment and
    /// would lock everyone out. (Deferral 1.)
    pub secure_cookie: bool,
    /// The console handler events (and the access middleware) emit through.
    /// Defaults to [`Console::noop`] so the flip and tests stay silent; the CLI
    /// serve paths replace it with a real stdout console via [`with_console`].
    pub console: Console,
    /// Whether the per-request access log is on (`[server] access_log`). Off by
    /// default; the CLI serve paths set it from config.
    pub access_log: bool,
    /// Cap on concurrent `/ws` connections (`[server] max_websocket_connections`).
    /// Defaults to [`dux_core::config::DEFAULT_MAX_WEBSOCKET_CONNECTIONS`]; the
    /// serve paths override it from config via [`with_max_websocket_connections`].
    pub max_websocket_connections: u32,
}

impl RouterParams {
    /// Plain-HTTP defaults: production recheck cadence, NO Secure cookie, a no-op
    /// console, no access log. Used by the loopback/Tailscale/proxy serve paths
    /// and every test harness; the CLI paths layer a real console on with
    /// [`with_console`].
    pub fn plain_http() -> Self {
        Self {
            ws_recheck_period: WS_RECHECK_PERIOD,
            secure_cookie: false,
            console: Console::noop(),
            access_log: false,
            max_websocket_connections: dux_core::config::DEFAULT_MAX_WEBSOCKET_CONNECTIONS,
        }
    }

    /// TLS (ACME) defaults: production recheck cadence, Secure cookie ON because
    /// dux terminates HTTPS so the browser will always send it back. No-op
    /// console + no access log by default (the CLI path layers them on).
    pub fn tls() -> Self {
        Self {
            ws_recheck_period: WS_RECHECK_PERIOD,
            secure_cookie: true,
            console: Console::noop(),
            access_log: false,
            max_websocket_connections: dux_core::config::DEFAULT_MAX_WEBSOCKET_CONNECTIONS,
        }
    }

    /// Attach a real console + the access-log toggle. The CLI serve paths
    /// (`run_plain_http`/`run_acme`) call this so handler events and the access
    /// middleware reach stdout; the flip leaves the no-op console in place.
    pub fn with_console(mut self, console: Console, access_log: bool) -> Self {
        self.console = console;
        self.access_log = access_log;
        self
    }

    /// Set the concurrent-connection cap from `[server] max_websocket_connections`.
    /// The serve paths call this so the configured value (not just the default)
    /// bounds live sockets.
    pub fn with_max_websocket_connections(mut self, max: u32) -> Self {
        self.max_websocket_connections = max;
        self
    }
}

/// Like [`router_with_auth`] but with an injectable `/ws` recheck period so the
/// revocation e2e can drive a short window instead of the production cadence.
/// Plain-HTTP defaults otherwise.
pub fn build_router_with_recheck(
    engine: EngineHandle,
    auth: SharedAuth,
    extra_gated: Router<AppState>,
    ws_recheck_period: std::time::Duration,
) -> Router {
    build_app(
        engine,
        auth,
        extra_gated,
        RouterParams {
            ws_recheck_period,
            secure_cookie: false,
            console: Console::noop(),
            access_log: false,
            max_websocket_connections: dux_core::config::DEFAULT_MAX_WEBSOCKET_CONNECTIONS,
        },
    )
    .0
}

/// Shared router builder, returning BOTH the router and the session store so a
/// caller can run the periodic expired-session sweep against it (the store is an
/// `Arc` clone, so sweeping the returned handle prunes the SAME map the router
/// uses). `extra_gated` is merged INTO the gated sub-router before the gate
/// middleware is layered on, so any route it carries inherits the session gate
/// exactly as `/ws` does. Production callers pass an empty router; a test injects
/// a probe route to prove the gate covers arbitrary data routes, not just `/ws`
/// (see [`tests::gated_data_route_is_401_without_session`]).
pub fn build_app(
    engine: EngineHandle,
    auth: SharedAuth,
    extra_gated: Router<AppState>,
    params: RouterParams,
) -> (Router, SweepableMemoryStore) {
    // In-memory session store: sessions die with the server (documented v1
    // limitation — a restart forces re-login). It is a `SweepableMemoryStore`
    // rather than tower-sessions' `MemoryStore` because that store never evicts
    // EXPIRED records (its `load` skips them, but they linger in the map until the
    // process exits). The sweepable store implements `ExpiredDeletion`; the serve
    // paths spawn a periodic `delete_expired` sweep against the returned handle so
    // memory stays bounded on a long-lived server with login churn. (Deferral 3.)
    // Returned alongside the router so the caller owns that sweep handle.
    let store = SweepableMemoryStore::new();
    let router = build_app_with_store(engine, auth, extra_gated, params, store.clone());
    (router, store)
}

/// Store-injectable core of [`build_app`]. Production always uses [`build_app`],
/// which creates a [`SweepableMemoryStore`] and returns it so the serve paths can
/// run the expiry sweep against it; this generic form lets an in-crate test inject
/// a `tower_sessions::SessionStore` that fails on demand to exercise the handlers'
/// session-error branches (e.g. a `cycle_id`/`insert` failure during login, which
/// must still refund the rate-limit charge). The `store` is consumed by the session
/// layer, so a caller that needs the sweep handle (see [`build_app`]) must clone its
/// `Arc`-backed store before passing it here. `Clone` is required because
/// `Router::layer` needs the session service to be cloneable. `pub(crate)`: a test
/// seam, not part of the crate's public API.
pub(crate) fn build_app_with_store<S>(
    engine: EngineHandle,
    auth: SharedAuth,
    extra_gated: Router<AppState>,
    params: RouterParams,
    store: S,
) -> Router
where
    S: tower_sessions::SessionStore + Clone + 'static,
{
    // A zero cap is a valid-but-drastic setting ("refuse all new connections").
    // Warn loudly at startup so an accidental 0 isn't a silent web-UI lock-out —
    // every upgrade would 503 with no other clue (explicit failure over silence).
    if params.max_websocket_connections == 0 {
        dux_core::logger::warn(
            "[server] max_websocket_connections = 0: every WebSocket upgrade will be \
             refused with HTTP 503 and the web UI will be unreachable",
        );
    }
    // The event bus and changed-files service are web-layer concerns built here.
    // `ChangesService::new` spawns its supervised poller, so this must run inside a
    // tokio runtime context — the CLI serve paths build inside `block_on`, and the
    // flip wraps its `build_app` in `runtime.enter()` (see `serve_with_engine`).
    let event_bus = Arc::new(EventBus::new());
    let changes = ChangesService::new(engine.clone(), Arc::clone(&event_bus));
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
    // so clients on the `projects` / `sessions` topics refetch `/api/v1/spine`.
    // Same lifetime/teardown story as the config forwarder above.
    spawn_spine_changed_forwarder(engine.subscribe_spine_changes(), Arc::clone(&event_bus));
    let state = AppState {
        engine,
        auth,
        rate_limiter: RateLimiter::default(),
        ws_recheck_period: params.ws_recheck_period,
        console: params.console,
        access_log: params.access_log,
        ws_semaphore: Arc::new(tokio::sync::Semaphore::new(
            params.max_websocket_connections as usize,
        )),
        event_bus,
        changes,
        idempotency: Arc::new(crate::rest_common::IdempotencyCache::new()),
        pty_size_owners: Arc::new(PtySizeOwners::default()),
    };

    // HttpOnly and SameSite=Strict are the tower-sessions defaults but we set them
    // explicitly so the intent is visible and a future default change can't
    // silently weaken the cookie.
    let session_layer = SessionManagerLayer::new(store)
        .with_name(auth::SESSION_COOKIE_NAME)
        .with_http_only(true)
        .with_same_site(SameSite::Strict)
        // Secure attribute is set ONLY when dux terminates TLS (the ACME path).
        // On plain HTTP a Secure cookie would never be sent back over the
        // loopback/proxy deployment, locking everyone out — so it stays off
        // there. (Deferral 1; the flag is decided by the caller via RouterParams.)
        .with_secure(params.secure_cookie)
        .with_expiry(Expiry::OnInactivity(CookieDuration::days(
            auth::SESSION_INACTIVITY_DAYS,
        )));

    // GATED routes: the gate middleware runs before these. By convention, add
    // every new data route here so it inherits the session requirement — the
    // structure can't stop a route from being misplaced into the open group.
    let gated = Router::new()
        .route("/ws/events", get(ws_events_upgrade))
        // Nested per-PTY byte-stream sockets. One socket per attached PTY: the
        // agent session's main provider PTY and a companion terminal's PTY. Each
        // replicates the four WS protections in its upgrade handler.
        .route("/ws/sessions/{id}/pty", get(ws_session_pty_upgrade))
        .route(
            "/ws/sessions/{id}/terminals/{tid}/pty",
            get(ws_terminal_pty_upgrade),
        )
        .merge(crate::git_routes::routes())
        .merge(crate::file_routes::routes())
        .merge(crate::changes_routes::routes())
        .merge(crate::bootstrap_routes::routes())
        .merge(crate::spine_routes::routes())
        .merge(crate::session_actions::routes())
        .merge(crate::project_actions::routes())
        .merge(crate::project_reads::routes())
        .merge(crate::startup_logs::routes())
        .merge(crate::terminal_actions::routes())
        .merge(crate::browse_routes::routes())
        .merge(crate::config_routes::routes())
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
        // The access log is the OUTERMOST layer OF THIS app, so it sees the final
        // status every layer it wraps produced — a 401 from the gate, the static
        // fallback's status. It is gated inside on `access_log && console.is_active`,
        // so the flip and disabled-console paths pay nothing. Stamped via
        // `from_fn_with_state` so it reads the console/toggle off `AppState`.
        //
        // CAVEAT on the TLS (:443) path: `run_acme` wraps the host-allowlist and
        // HSTS layers OUTSIDE this app (see `tls::host_allowlist_layer`), so a
        // foreign-Host probe is 421'd by the allowlist BEFORE it reaches this
        // middleware — those 421s are deliberately NOT access-logged on :443, which
        // keeps a rebinding-probe flood out of the log. (The `:80`
        // challenge/redirect router DOES log its 421s: there the access log is the
        // true outermost layer — see `tls::build_http_challenge_router`.)
        .layer(middleware::from_fn_with_state(state.clone(), access_log))
        .with_state(state)
}

/// Per-request access-log middleware for the main app: print
/// `method path status latencyms` to the console after the inner stack produces a
/// response. Reads the console + toggle off [`AppState`] and delegates to the
/// shared [`log_request`] core (the SAME core the `:80` challenge/redirect router
/// uses via [`access_log_layer`], so both surfaces log identically).
async fn access_log(State(state): State<AppState>, request: Request, next: Next) -> Response {
    log_request(&state.console, state.access_log, request, next).await
}

/// The shared access-log core. CONSOLE-ONLY (never `dux.log` — piping
/// `dux server`'s stdout IS the access log). Skips `/healthz` so a health checker
/// does not flood the log, and is gated on `access_log && console.is_active()` so
/// the flip/disabled paths emit nothing.
///
/// The path is printed WITHOUT its query string. Query parameters can carry
/// sensitive values — `GET /api/file/raw?session_id=…&path=…` puts the session
/// cookie id and an absolute filesystem path in the query — and this log is the
/// `dux server` stdout an operator may forward to a file or aggregator, so the
/// query is dropped to avoid leaking secrets. The ACME challenge token rides the
/// PATH (`/.well-known/acme-challenge/{token}`), not the query, so challenge
/// fetches are still visible.
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
    // (e.g. /api/file/raw?session_id=…&path=…), and this log is stdout an operator
    // may persist; the ACME challenge token is in the path, so dropping the query
    // loses nothing useful.
    let path = request.uri().path().to_string();
    let started = std::time::Instant::now();
    let response = next.run(request).await;
    let latency_ms = started.elapsed().as_millis();
    console.access(&method, &path, response.status().as_u16(), latency_ms);
    response
}

/// The console + access-log toggle carried as middleware state for routers that
/// have no [`AppState`] — namely the `:80` ACME challenge/redirect router (see
/// [`access_log_layer`]). Cheap to clone (the `Console` is `Arc`-backed).
#[derive(Clone)]
struct AccessLogState {
    console: Console,
    access_log: bool,
}

/// Wrap a router with the shared access-log middleware, carrying the console +
/// toggle explicitly (no [`AppState`] required). Used to give the `:80`
/// challenge/redirect router the SAME access log as the main app, so a challenge
/// fetch, a 308 redirect, a 421 foreign-Host rejection, or a 400 all log one line
/// like any other request. Gated identically (`access_log && console.is_active()`),
/// so the flip and disabled-console paths emit nothing.
pub fn access_log_layer(router: Router, console: Console, access_log: bool) -> Router {
    let state = AccessLogState {
        console,
        access_log,
    };
    router.layer(middleware::from_fn_with_state(
        state,
        |State(state): State<AccessLogState>, request: Request, next: Next| async move {
            log_request(&state.console, state.access_log, request, next).await
        },
    ))
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
// DNS-rebinding defense: the same-origin check below trusts the request's own
// `Host` header, so on its own it does not stop a rebinding attacker who points a
// controlled hostname at this server's IP (the browser then sends a matching
// Origin/Host pair). When dux terminates TLS (the ACME path), the
// `crate::tls::host_allowlist_layer` middleware runs AHEAD of the gate on the
// whole HTTPS app and pins the accepted `Host` values to the configured domains,
// closing that gap (a mismatched Host gets 421 before reaching here). The plain
// HTTP path has no allowlist by design — loopback/proxy mode, where the proxy
// owns Host hygiene — so this same-origin check remains the WS-specific defense
// there. (Deferral 2.)
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
/// [`same_origin_allowed`] is authority-only — it does NOT distinguish an
/// `http://` Origin from an `https://` one for the same host. A cross-protocol
/// upgrade is not blocked here; browsers reject it via mixed-content policy, and
/// on the TLS path the host allowlist is the complementary layer.
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

/// Read the username to periodically recheck for revocation on an authenticated
/// socket (the HTTP gate cannot revoke an already-upgraded socket). `None` when
/// auth is disabled or no username is recorded — such sockets are never rechecked.
/// Shared by the nested PTY upgrade handlers so they recheck identically to `/ws`.
async fn ws_recheck_user(state: &AppState, session: &Session) -> Option<String> {
    if auth::is_enabled(&state.auth) {
        session
            .get::<String>(auth::SESSION_USER_KEY)
            .await
            .ok()
            .flatten()
    } else {
        None
    }
}

/// Acquire a connection-cap permit before a WS upgrade. `None` means the cap is
/// exhausted (the caller responds 503); a refusal is logged here with `route` so an
/// operator can see which endpoint hit the cap. The permit moves into the socket
/// task and frees the slot when the task returns, so the cap bounds the COMBINED
/// live socket count across every WS family (see [`AppState::ws_semaphore`]).
/// Returns `Option` rather than `Result<_, Response>` so the large `Response` does
/// not bloat the `Err` variant (clippy `result_large_err`).
fn acquire_ws_permit(
    state: &AppState,
    peer_ip: std::net::IpAddr,
    route: &str,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    match Arc::clone(&state.ws_semaphore).try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            dux_core::logger::warn(&format!(
                "[server] {route} upgrade refused for {peer_ip}: connection cap reached \
                 (max_websocket_connections)"
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
}

impl PtyTarget {
    /// The id used to route stdin writes and resizes (the session id for an agent,
    /// the terminal id for a companion terminal). The engine's `pty_for` accepts
    /// either keyspace.
    fn pty_id(&self) -> &str {
        match self {
            PtyTarget::Agent(id) | PtyTarget::Terminal(id) => id,
        }
    }
}

/// A resize control frame on a PTY socket: the Text frame `{"rows":R,"cols":C}`,
/// distinct from the Binary stdin frames. Routed to `engine.resize_pty` for the
/// socket's own PTY ONLY when this connection currently owns sizing for it (see
/// [`PtySizeOwners`]); a non-owner's resize is ignored so two viewers of one PTY
/// don't thrash its size last-writer-wins.
#[derive(serde::Deserialize)]
struct PtyResizeFrame {
    rows: u16,
    cols: u16,
}

/// Tracks which connection currently owns sizing for each PTY, keyed by pty id
/// (the session id for an agent PTY, the terminal id for a companion). The most
/// recently ATTACHED connection owns sizing; a resize from a non-owner is ignored,
/// which breaks the last-writer-wins feedback loop two viewers of one PTY would
/// otherwise create. Lives in [`AppState`] so every per-PTY socket shares it.
#[derive(Default)]
struct PtySizeOwners {
    owners: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    next_conn_id: std::sync::atomic::AtomicU64,
}

impl PtySizeOwners {
    /// Allocate a process-unique id for a freshly attached PTY socket, used to
    /// compare against the recorded owner.
    fn next_conn_id(&self) -> u64 {
        self.next_conn_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Make `conn_id` the current sizing owner of `pty_id`. Called on every new
    /// attach, so the most recently attached connection wins.
    fn claim(&self, pty_id: &str, conn_id: u64) {
        self.owners
            .lock()
            .unwrap()
            .insert(pty_id.to_string(), conn_id);
    }

    /// Whether `conn_id` may apply a resize to `pty_id`: true if it is the current
    /// owner, or if no owner is recorded (the previous owner dropped) — in which
    /// case `conn_id` claims it so the next live resize takes over.
    fn may_resize(&self, pty_id: &str, conn_id: u64) -> bool {
        let mut owners = self.owners.lock().unwrap();
        match owners.get(pty_id) {
            Some(&owner) => owner == conn_id,
            None => {
                owners.insert(pty_id.to_string(), conn_id);
                true
            }
        }
    }

    /// Release ownership of `pty_id` if `conn_id` still holds it (called when the
    /// connection disconnects). A no-op if another connection has since claimed it,
    /// so a later attach is never clobbered.
    fn release(&self, pty_id: &str, conn_id: u64) {
        let mut owners = self.owners.lock().unwrap();
        if owners.get(pty_id) == Some(&conn_id) {
            owners.remove(pty_id);
        }
    }
}

/// Upgrade handler for `GET /ws/sessions/:id/pty` — stream the agent session's main
/// provider PTY. Replicates the four `/ws` protections (origin check, connection-cap
/// permit, frame-size limit, user-revocation recheck) and path-validates `:id`
/// against a known session (404 otherwise, before the upgrade).
async fn ws_session_pty_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    session: Session,
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
    let recheck_user = ws_recheck_user(&state, &session).await;
    let permit = match acquire_ws_permit(&state, peer.ip(), "/ws/sessions/:id/pty") {
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
    let auth = state.auth.clone();
    let recheck_period = state.ws_recheck_period;
    let console = state.console.clone();
    let pty_size_owners = Arc::clone(&state.pty_size_owners);
    let peer_ip = peer.ip();
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_pty_socket(
                socket,
                engine,
                PtyTarget::Agent(id),
                auth,
                recheck_user,
                recheck_period,
                console,
                peer_ip,
                permit,
                pty_size_owners,
            )
        })
        .into_response()
}

/// Upgrade handler for `GET /ws/sessions/:id/terminals/:tid/pty` — stream a
/// companion terminal's PTY. Same four protections as the agent socket, and
/// path-validates BOTH that `:id` is a known session AND that `:tid` belongs to it
/// (the legacy `SubscribeTerminal` looked terminals up by id alone; here the path
/// enforces session ownership). Either failing is a 404 before the upgrade.
async fn ws_terminal_pty_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path((id, tid)): Path<(String, String)>,
    session: Session,
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
    // Enforce that the terminal belongs to THIS session: an unknown terminal, or
    // one owned by a different session, is a 404 (never a cross-session attach).
    match state.engine.terminal_session(tid.clone()).await {
        Some(owner) if owner == id => {}
        _ => return (StatusCode::NOT_FOUND, "unknown terminal").into_response(),
    }
    let recheck_user = ws_recheck_user(&state, &session).await;
    let permit = match acquire_ws_permit(&state, peer.ip(), "/ws/sessions/:id/terminals/:tid/pty") {
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
    let auth = state.auth.clone();
    let recheck_period = state.ws_recheck_period;
    let console = state.console.clone();
    let pty_size_owners = Arc::clone(&state.pty_size_owners);
    let peer_ip = peer.ip();
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_pty_socket(
                socket,
                engine,
                PtyTarget::Terminal(tid),
                auth,
                recheck_user,
                recheck_period,
                console,
                peer_ip,
                permit,
                pty_size_owners,
            )
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
    auth: SharedAuth,
    recheck_user: Option<String>,
    recheck_period: std::time::Duration,
    console: Console,
    peer_ip: std::net::IpAddr,
    // Held for the socket's lifetime purely for its Drop (frees a connection-cap
    // slot when this returns). Never read.
    _permit: tokio::sync::OwnedSemaphorePermit,
    pty_size_owners: Arc<PtySizeOwners>,
) {
    console.client_connected(peer_ip);
    let (sink, mut stream) = socket.split();
    let sink: SharedSink = Arc::new(tokio::sync::Mutex::new(sink));

    // Subscribe to the target PTY. An agent subscribe also launches/resumes the
    // provider if it isn't running yet (the same flow the legacy Subscribe uses);
    // a terminal subscribe attaches to an already-created companion terminal.
    let subscription = match &target {
        PtyTarget::Agent(id) => engine.subscribe_pty(id.clone()).await,
        PtyTarget::Terminal(id) => engine.subscribe_terminal(id.clone()).await,
    };
    let (repaint, rx) = match subscription {
        Ok(sub) => sub,
        Err(e) => {
            // Subscribe failed after the upgrade (e.g. the agent failed to launch,
            // or the terminal vanished in the gap). Best-effort Close, then exit.
            dux_core::logger::warn(&format!(
                "PTY socket subscribe failed for {peer_ip} (pty {:?}): {e}",
                target.pty_id()
            ));
            {
                let mut guard = sink.lock().await;
                let _ = guard.send(Message::Close(None)).await;
            }
            console.client_disconnected(peer_ip);
            return;
        }
    };
    // This connection now owns sizing for its PTY: the most recently attached socket
    // wins, so a second viewer opening takes over and the first viewer's later
    // resizes are ignored until it reattaches. Released on disconnect below.
    let conn_id = pty_size_owners.next_conn_id();
    pty_size_owners.claim(target.pty_id(), conn_id);
    // Replay the buffered scrollback/repaint before streaming live bytes.
    send_binary(&sink, repaint).await;
    let pty_forwarder = spawn_pty_forwarder(Arc::clone(&sink), rx, engine.shutdown_flag());

    // Periodic user-existence recheck (auth-on sockets only), same policy as `/ws`.
    let mut recheck = tokio::time::interval(recheck_period);
    if recheck_user.is_some() {
        recheck.tick().await;
    }

    loop {
        let msg = tokio::select! {
            _ = recheck.tick(), if recheck_user.is_some() => {
                if !auth::is_enabled(&auth) {
                    // Gate downgraded to OFF: a loosening, not a revocation. Keep streaming.
                    continue;
                }
                let still_valid = recheck_user
                    .as_deref()
                    .map(|u| auth::username_exists(&auth, u))
                    .unwrap_or(false);
                if !still_valid {
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
            // Binary frame = raw PTY stdin for THIS socket's PTY.
            Message::Binary(bytes) => {
                engine.write_pty(target.pty_id().to_string(), bytes.to_vec());
            }
            // Text frame = a resize control message. Applied to the engine PTY ONLY
            // when this connection currently owns sizing (the most recently attached
            // viewer). A non-owner's resize is dropped, breaking the last-writer-wins
            // feedback loop between two viewers of the same PTY.
            Message::Text(text) => {
                if let Ok(frame) = serde_json::from_str::<PtyResizeFrame>(text.as_str())
                    && pty_size_owners.may_resize(target.pty_id(), conn_id)
                {
                    engine.resize_pty(target.pty_id().to_string(), frame.rows, frame.cols);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Detach: stop the forwarder so it doesn't linger on the subscription, and
    // release sizing ownership so the next attach (or the next live resize) claims it.
    pty_forwarder.abort();
    pty_size_owners.release(target.pty_id(), conn_id);
    console.client_disconnected(peer_ip);
}

/// The single `config.changed` signal emitted whenever the engine reloads config.
/// No `id`/`rev` — it is a plain "refetch `/api/v1/bootstrap`" signal delivered on
/// the coarse `config` topic.
fn config_changed_event() -> Event {
    Event::Resource {
        event: "config.changed".to_string(),
        id: None,
        rev: None,
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
/// read" (`/api/v1/projects` or `/api/v1/spine`), delivered on the `projects` topic.
fn projects_changed_event() -> Event {
    Event::Resource {
        event: "projects.changed".to_string(),
        id: None,
        rev: None,
    }
}

/// A coarse `sessions.changed` signal: no `id`/`rev`, just "refetch the sessions
/// read" (`/api/v1/sessions` or `/api/v1/spine`), delivered on the `sessions` topic.
/// Covers session lifecycle/status, the `working` flag, and the terminal list in
/// Phase 3 (they all live in the sessions/sidebar projection).
fn sessions_changed_event() -> Event {
    Event::Resource {
        event: "sessions.changed".to_string(),
        id: None,
        rev: None,
    }
}

/// Bridge engine spine changes onto the event bus as coarse `projects.changed` /
/// `sessions.changed` events. The engine loop fires a [`SpineChange`] per changed
/// side; this task re-emits the matching event so subscribed clients refetch
/// `/api/v1/spine`. On `Lagged` it re-emits BOTH coarse signals once (the signals
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
}

/// Outbound `/ws/events` status event: the one event carrying an inline payload
/// (a toast has nothing to GET). Shape:
/// `{ "event": "status", "key": "op-7", "tone": "info", "message": "…", "scope": "all" }`.
/// The server has already filtered on `scope`, but it is serialized for wire
/// parity so a client may render/correlate it.
#[derive(serde::Serialize)]
struct WireStatusEvent {
    event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    tone: String,
    message: String,
    scope: StatusScope,
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

/// Upgrade handler for `/ws/events`. Replicates the four WS protections (origin
/// check, connection-cap permit, frame-size limit, user-revocation recheck). The
/// per-connection `connection_id` (minted inside [`handle_events_socket`]) is sent
/// as the first frame and drives status-toast scoping: a REST action echoes it via
/// `X-Connection-Id` so its status reaches only the originating connection.
async fn ws_events_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    session: Session,
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
    let recheck_user = if auth::is_enabled(&state.auth) {
        session
            .get::<String>(auth::SESSION_USER_KEY)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let permit = match Arc::clone(&state.ws_semaphore).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            dux_core::logger::warn(&format!(
                "[server] /ws/events upgrade refused for {}: connection cap reached \
                 (max_websocket_connections)",
                peer.ip()
            ));
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "too many WebSocket connections; try again shortly",
            )
                .into_response();
        }
    };
    let auth = state.auth.clone();
    let recheck_period = state.ws_recheck_period;
    let console = state.console.clone();
    let engine = state.engine.clone();
    let bus = Arc::clone(&state.event_bus);
    let changes = Arc::clone(&state.changes);
    let peer_ip = peer.ip();
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .on_upgrade(move |socket| {
            handle_events_socket(
                socket,
                engine,
                bus,
                changes,
                auth,
                recheck_user,
                recheck_period,
                console,
                peer_ip,
                permit,
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
    auth: SharedAuth,
    recheck_user: Option<String>,
    recheck_period: std::time::Duration,
    console: Console,
    peer_ip: std::net::IpAddr,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    console.client_connected(peer_ip);
    // A server-assigned random id correlating REST actions with the statuses they
    // mint, so an operation's toasts (push/commit/launch) are delivered ONLY back
    // to the originating connection (`StatusScope::Connection`). The client echoes
    // it as the `X-Connection-Id` header on REST mutations. Never client-supplied.
    let connection_id = uuid::Uuid::new_v4().to_string();
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

    let mut recheck = tokio::time::interval(recheck_period);
    if recheck_user.is_some() {
        recheck.tick().await;
    }

    loop {
        tokio::select! {
            // User-revocation recheck (auth-on sockets only).
            _ = recheck.tick(), if recheck_user.is_some() => {
                if !auth::is_enabled(&auth) {
                    continue;
                }
                let still_valid = recheck_user
                    .as_deref()
                    .map(|u| auth::username_exists(&auth, u))
                    .unwrap_or(false);
                if !still_valid {
                    let mut guard = sink.lock().await;
                    let _ = guard.send(Message::Close(None)).await;
                    break;
                }
                continue;
            }
            ev = bus_rx.recv() => match ev {
                Ok(Event::Resource { event, id, rev }) => {
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
                        // Coarse spine signals ride their own coarse topics (no
                        // id/rev — a plain refetch of `/api/v1/spine`).
                        ("projects.changed", _) => interest.subscribed.contains("projects"),
                        ("sessions.changed", _) => interest.subscribed.contains("sessions"),
                        _ => false,
                    };
                    if deliver {
                        let frame = WireEvent { event, id, rev };
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
                    // for each held fine topic (never back onto the broadcast bus).
                    let mut sink_dead = false;
                    for topic in interest.subscribed.iter() {
                        if let Some(sid) = event_bus::session_id_from_changes_topic(topic) {
                            let frame = WireEvent {
                                event: "session.changes".to_string(),
                                id: Some(sid.to_string()),
                                rev: changes.peek_rev(sid),
                            };
                            if send_event(&sink, &frame).await.is_err() {
                                sink_dead = true;
                                break;
                            }
                        }
                    }
                    if sink_dead {
                        break;
                    }
                    // The coarse `config` topic carries no per-resource rev, so the
                    // fine-topic loop above never covers it. A lagged client holding
                    // `config` would keep stale bootstrap unless we explicitly tell it
                    // to refetch; emit one `config.changed` directly to this sink
                    // (mirroring how `spawn_config_changed_forwarder` recovers).
                    if interest.subscribed.contains("config") {
                        let frame = WireEvent {
                            event: "config.changed".to_string(),
                            id: None,
                            rev: None,
                        };
                        if send_event(&sink, &frame).await.is_err() {
                            break;
                        }
                    }
                    // The coarse `projects`/`sessions` topics likewise carry no
                    // per-resource rev, so a lagged client holding them needs an
                    // explicit refetch nudge to recover from stale spine data.
                    if interest.subscribed.contains("projects") {
                        let frame = WireEvent {
                            event: "projects.changed".to_string(),
                            id: None,
                            rev: None,
                        };
                        if send_event(&sink, &frame).await.is_err() {
                            break;
                        }
                    }
                    if interest.subscribed.contains("sessions") {
                        let frame = WireEvent {
                            event: "sessions.changed".to_string(),
                            id: None,
                            rev: None,
                        };
                        if send_event(&sink, &frame).await.is_err() {
                            break;
                        }
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
                    // Recover the missed updates by replaying the current scoped
                    // status snapshot. The client reconciles by key (it replaces the
                    // toast for each open status), so missed live updates are healed.
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
                    // A dropped clear cannot be reconstructed directly, so re-send the
                    // snapshot of statuses still open: the client reconciles by key
                    // (the frontend re-syncs to this set), recovering from a missed
                    // dismissal for any keyed toast no longer present.
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
                        apply_events_frame(&frame, &mut interest.subscribed, &engine, &bus).await;
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
async fn apply_events_frame(
    frame: &EventsClientFrame,
    subscribed: &mut std::collections::HashSet<String>,
    engine: &EngineHandle,
    bus: &EventBus,
) {
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
        return;
    }

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
                }
            }
            // A coarse topic (sessions/projects/config): tracked for forwarding,
            // but it carries no poll interest in Phase 1.
            None => {
                subscribed.insert(topic.clone());
            }
        }
    }
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

/// Re-send the current scoped status snapshot to one connection. Used to recover
/// after a `Lagged` on either the status or status-clear broadcast: the client
/// reconciles by key (replacing the toast for each still-open status), so missed
/// live updates and dismissals are healed without the server diffing. Returns
/// `Err(())` if the sink is dead so the caller can break the connection loop.
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
/// Each open `KeyedWireStatus` in `snapshot` (non-empty message) whose scope is
/// deliverable to `conn_id` maps to one [`WireStatusEvent`]. The scope filter
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
        })
        .collect()
}

async fn send_binary(sink: &SharedSink, bytes: Vec<u8>) {
    let mut guard = sink.lock().await;
    let _ = guard.send(Message::Binary(bytes.into())).await;
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

    /// A session store that delegates to a real in-memory store but can be ARMED to
    /// fail `delete` on demand. `delete` is the store call `Session::cycle_id` makes
    /// during login: on a fresh (cookieless) login there is no persisted prior
    /// session, but tower-sessions still calls `store.delete` UNCONDITIONALLY on the
    /// freshly-generated id (see `session.rs` `cycle_id`). Arming this therefore makes
    /// `session.cycle_id().await` in the login handler return `Err` — the exact
    /// "correct password, then session error" branch we need. Injected via
    /// [`build_app_with_store`].
    #[derive(Clone, Debug)]
    struct FaultOnDeleteStore {
        inner: SweepableMemoryStore,
        fail_delete: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl FaultOnDeleteStore {
        fn new() -> Self {
            Self {
                inner: SweepableMemoryStore::new(),
                fail_delete: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }
        /// Make every subsequent `delete` fail.
        fn arm(&self) {
            self.fail_delete
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl tower_sessions::session_store::SessionStore for FaultOnDeleteStore {
        async fn create(
            &self,
            record: &mut tower_sessions::session::Record,
        ) -> tower_sessions::session_store::Result<()> {
            tower_sessions::session_store::SessionStore::create(&self.inner, record).await
        }
        async fn save(
            &self,
            record: &tower_sessions::session::Record,
        ) -> tower_sessions::session_store::Result<()> {
            tower_sessions::session_store::SessionStore::save(&self.inner, record).await
        }
        async fn load(
            &self,
            id: &tower_sessions::session::Id,
        ) -> tower_sessions::session_store::Result<Option<tower_sessions::session::Record>>
        {
            tower_sessions::session_store::SessionStore::load(&self.inner, id).await
        }
        async fn delete(
            &self,
            id: &tower_sessions::session::Id,
        ) -> tower_sessions::session_store::Result<()> {
            if self.fail_delete.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(tower_sessions::session_store::Error::Backend(
                    "injected delete failure".to_string(),
                ));
            }
            tower_sessions::session_store::SessionStore::delete(&self.inner, id).await
        }
    }

    /// The ghost-charge ordering fix, exercised end-to-end: a CORRECT password whose
    /// session commit then fails must STILL refund the rate-limit charge. The
    /// handler charges before the bcrypt verify and calls `clear()` immediately
    /// after a correct verify — BEFORE `cycle_id()`/`insert()` — so a session error
    /// returns 500 without leaving the attempt counted as a failure. If `clear()`
    /// ran after the session ops (the pre-fix ordering) the budget would be stuck at
    /// the limit and the next attempt would 429. This faults `cycle_id`'s `delete`;
    /// the `insert` branch is symmetric (the single `clear()` precedes both ops), so
    /// one fault path proves the ordering for both.
    #[tokio::test]
    async fn correct_login_with_session_error_still_refunds_the_budget() {
        fn login_req(user: &str, pw: &str) -> axum::http::Request<axum::body::Body> {
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/login")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(format!(
                    r#"{{"username":"{user}","password":"{pw}"}}"#
                )))
                .unwrap()
        }

        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let hash = dux_core::auth::hash_password("secret-pw").unwrap();
        let auth = auth::shared_auth(&[format!("alice:{hash}")], false);

        let store = FaultOnDeleteStore::new();
        // The login handler needs ConnectInfo<SocketAddr> for the per-IP limiter;
        // MockConnectInfo supplies a fixed peer so every request shares one bucket.
        let peer: SocketAddr = "10.9.9.9:4242".parse().unwrap();
        let app = build_app_with_store(
            handle,
            auth,
            Router::new(),
            RouterParams::plain_http(),
            store.clone(),
        )
        .layer(axum::extract::connect_info::MockConnectInfo(peer));

        // (max-1) wrong logins → budget at exactly one below the limit, so the next
        // (correct) login charges to the limit. Referencing the constant — rather
        // than a bare `4` — keeps the test discriminating at ANY value of
        // RATE_LIMIT_MAX_FAILURES: a literal would go vacuous if the limit were
        // raised (the correct login would no longer sit at the limit, so a missing
        // refund wouldn't 429). Wrong logins 401 before any session op, so they
        // never touch the store.
        for _ in 0..(auth::RATE_LIMIT_MAX_FAILURES - 1) {
            let resp = app
                .clone()
                .oneshot(login_req("alice", "WRONG"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        // Arm the store so cycle_id()'s delete fails during the next (correct) login.
        store.arm();

        // Correct password: charges to the limit, bcrypt ok, clear() refunds (→0),
        // then cycle_id() hits the failing delete → 500.
        let errored = app
            .clone()
            .oneshot(login_req("alice", "secret-pw"))
            .await
            .unwrap();
        assert_eq!(
            errored.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a session error after a correct password must surface as 500"
        );

        // The next wrong login must be a normal 401, NOT 429 — proof the correct
        // login refunded its charge despite the session error.
        let after = app
            .clone()
            .oneshot(login_req("alice", "WRONG"))
            .await
            .unwrap();
        assert_eq!(
            after.status(),
            StatusCode::UNAUTHORIZED,
            "a correct login that hit a session error must still refund its rate-limit \
             charge (got {} — the budget was not refunded)",
            after.status()
        );
    }

    /// A representative data route added to the GATED sub-router must 401 without
    /// a session when auth is on — proving the gate covers arbitrary gated data
    /// routes, not only `/ws`. This is the reviewer's regression test: it injects
    /// a probe route through `build_app`'s test seam so a future data route
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
        let app = build_app(handle, auth, probe, RouterParams::plain_http()).0;

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

    /// The real `GET /api/v1/sessions/:id/changes` read and the `/ws/events`
    /// upgrade are both in the gated group: unauthenticated → 401, before reaching
    /// the handler (the changed-files compute / the WS upgrade).
    #[tokio::test]
    async fn changes_and_events_routes_require_session() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let auth = auth::shared_auth(&[format!("alice:{hash}")], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

        let changes = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/sessions/s1/changes")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            changes.status(),
            StatusCode::UNAUTHORIZED,
            "the changed-files read must reject an unauthenticated request"
        );

        // The bootstrap read is in the same gated group: unauthenticated → 401.
        let bootstrap = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/bootstrap")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            bootstrap.status(),
            StatusCode::UNAUTHORIZED,
            "the bootstrap read must reject an unauthenticated request"
        );

        // The spine reads are in the same gated group: unauthenticated → 401.
        for uri in [
            "/api/v1/spine",
            "/api/v1/projects",
            "/api/v1/sessions",
            "/api/v1/sessions/s1",
        ] {
            let resp = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "the spine read {uri} must reject an unauthenticated request"
            );
        }

        // A plain GET to /ws/events (no Upgrade headers) still passes through the
        // gate first, which 401s before the WS upgrade is attempted.
        let events = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/ws/events")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            events.status(),
            StatusCode::UNAUTHORIZED,
            "the /ws/events upgrade must reject an unauthenticated request"
        );

        // The nested per-PTY sockets are in the same gated group: a plain GET (no
        // Upgrade headers) still passes through the gate first, which 401s before
        // any path validation or WS upgrade is attempted.
        for uri in ["/ws/sessions/s1/pty", "/ws/sessions/s1/terminals/t1/pty"] {
            let resp = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "the PTY socket {uri} must reject an unauthenticated request"
            );
        }
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
        let app = router(handle); // auth disabled → the gate passes through.

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
        let app = router(handle); // auth disabled → the gate passes through.

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

    /// A real git-mutation route is inside the gated group: unauthenticated →
    /// 401, before any git work runs.
    #[tokio::test]
    async fn git_route_requires_session() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let auth = auth::shared_auth(&[format!("alice:{hash}")], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/git/stage")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"session_id":"s1","path":"a.txt"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// With auth off the gate passes; an unknown session resolves to 404 (the
    /// handler is wired and resolves the worktree before doing any git work).
    #[tokio::test]
    async fn git_route_unknown_session_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/git/stage")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"session_id":"does-not-exist","path":"a.txt"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// The editor file routes live in the same gated group: unauthenticated read
    /// → 401 before any file I/O runs.
    #[tokio::test]
    async fn file_route_requires_session() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let auth = auth::shared_auth(&[format!("alice:{hash}")], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/file/read")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"session_id":"s1","path":"a.txt"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// With auth off the gate passes; an unknown session resolves to 404 (the
    /// write handler resolves the worktree before touching the filesystem).
    #[tokio::test]
    async fn file_route_unknown_session_is_404() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/file/write")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"session_id":"does-not-exist","path":"a.txt","content":"x"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
                    project_id: "p1".to_string(),
                    project_path: None,
                    provider: dux_core::model::ProviderKind::new("claude"),
                    source_branch: "main".to_string(),
                    branch_name: "feat".to_string(),
                    worktree_path: root.to_string_lossy().into_owned(),
                    title: None,
                    started_providers: Vec::new(),
                    desired_running: true,
                    auto_reopen_enabled: false,
                    status: dux_core::model::SessionStatus::Detached,
                    created_at: now,
                    updated_at: now,
                })
                .unwrap();
        }
        let engine = crate::bootstrap::bootstrap_engine(&paths).unwrap();
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        handle
    }

    /// `GET /api/v1/spine` returns the projects, sessions, and sidebar projection
    /// (auth off → the gate passes). Proves the spine read serves the same spine
    /// the ViewModel used to carry.
    #[tokio::test]
    async fn spine_route_returns_projects_sessions_and_sidebar() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/spine")
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
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

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

    /// Every Phase-4 session/project action route is in the gated group: an
    /// unauthenticated request 401s before the handler runs. Extends the gate
    /// regression to the new write verbs (and the `/api/v1` git/file aliases).
    #[tokio::test]
    async fn rest_action_routes_require_session() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let auth = auth::shared_auth(&[format!("alice:{hash}")], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

        let cases: &[(&str, &str, Option<&str>)] = &[
            (
                "POST",
                "/api/v1/sessions",
                Some(r#"{"kind":"new","project_id":"p1"}"#),
            ),
            ("DELETE", "/api/v1/sessions/s1", None),
            ("PATCH", "/api/v1/sessions/s1", Some(r#"{"title":"x"}"#)),
            ("POST", "/api/v1/sessions/s1/reconnect", Some("{}")),
            (
                "POST",
                "/api/v1/sessions/reorder",
                Some(r#"{"project_id":"p1","session_ids":[]}"#),
            ),
            ("POST", "/api/v1/projects", Some(r#"{"path":"/x"}"#)),
            ("DELETE", "/api/v1/projects/p1", None),
            ("PATCH", "/api/v1/projects/p1", Some("{}")),
            (
                "POST",
                "/api/v1/projects/reorder",
                Some(r#"{"project_ids":[]}"#),
            ),
            ("POST", "/api/v1/projects/p1/pull", None),
            ("POST", "/api/v1/projects/p1/checkout-default", None),
            // The /api/v1 git/file aliases are gated too.
            (
                "POST",
                "/api/v1/git/stage",
                Some(r#"{"session_id":"s1","path":"a"}"#),
            ),
            (
                "POST",
                "/api/v1/file/read",
                Some(r#"{"session_id":"s1","path":"a"}"#),
            ),
            // The Phase-5 companion-terminal verbs are gated too.
            ("POST", "/api/v1/sessions/s1/terminals", None),
            ("DELETE", "/api/v1/sessions/s1/terminals/t1", None),
        ];
        for (method, uri, body) in cases {
            assert_eq!(
                oneshot_status(&app, method, uri, *body).await,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must 401 without a session"
            );
        }
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
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

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
        // of everything under /api/v1/git.
        assert_ne!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/git/push",
                Some(r#"{"session_id":"nope"}"#)
            )
            .await,
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
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

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
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

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
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

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

    /// The `/api/v1` git/file routes reach their handlers: an unknown session
    /// resolves to 404 (auth off so the gate passes). The legacy unversioned
    /// `/api/git/*` and `/api/file/*` paths were removed at cutover, so they no
    /// longer reach the git/file handler (they fall through to the SPA fallback,
    /// which never returns the handler's 404).
    #[tokio::test]
    async fn v1_git_and_file_routes_reach_handlers() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

        assert_eq!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/git/stage",
                Some(r#"{"session_id":"nope","path":"a.txt"}"#)
            )
            .await,
            StatusCode::NOT_FOUND,
            "the v1 git route must reach the git handler and 404 the unknown session"
        );
        assert_eq!(
            oneshot_status(
                &app,
                "POST",
                "/api/v1/file/read",
                Some(r#"{"session_id":"nope","path":"a.txt"}"#)
            )
            .await,
            StatusCode::NOT_FOUND,
            "the v1 file route must reach the file handler and 404 the unknown session"
        );
        // The retired legacy paths no longer reach the handler (no 404 from it).
        assert_ne!(
            oneshot_status(
                &app,
                "POST",
                "/api/git/stage",
                Some(r#"{"session_id":"nope","path":"a.txt"}"#)
            )
            .await,
            StatusCode::NOT_FOUND,
            "the legacy /api/git/* alias must be gone"
        );
    }

    /// The literal `/reorder` segment does not collide with `:id` (a reorder with a
    /// full list against the seeded project is accepted — 200 — not routed into the
    /// `:id` handlers).
    #[tokio::test]
    async fn reorder_segment_does_not_collide_with_id() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = seeded_engine_handle(tmp.path());
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

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

    /// Two viewers of one PTY: the most recently attached connection owns sizing.
    /// The later attacher's resize applies; the earlier one's is ignored. After the
    /// owner drops, the surviving connection's next resize claims ownership.
    #[test]
    fn pty_size_owner_is_most_recent_attach_and_releases_on_drop() {
        let owners = PtySizeOwners::default();
        let pty = "session-1";

        // First viewer attaches and owns sizing.
        let conn_a = owners.next_conn_id();
        owners.claim(pty, conn_a);
        assert!(
            owners.may_resize(pty, conn_a),
            "the sole attached connection owns sizing"
        );

        // Second viewer attaches: it becomes the owner (most-recent-attach wins).
        let conn_b = owners.next_conn_id();
        owners.claim(pty, conn_b);
        assert!(
            owners.may_resize(pty, conn_b),
            "the later attacher's resize applies"
        );
        assert!(
            !owners.may_resize(pty, conn_a),
            "the earlier attacher's resize is ignored while it is not the owner"
        );

        // The owner (B) disconnects and releases ownership.
        owners.release(pty, conn_b);
        // Now A's next resize claims the unowned PTY and applies.
        assert!(
            owners.may_resize(pty, conn_a),
            "after the owner drops, the surviving connection claims sizing on its next resize"
        );
        // B's stale id no longer owns it.
        assert!(!owners.may_resize(pty, conn_b));
    }

    /// `release` is a no-op when another connection has already claimed the PTY, so
    /// a late-arriving disconnect from a former owner never steals sizing from the
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
        owners.release(pty, conn_a);
        assert!(
            owners.may_resize(pty, conn_b),
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
        let auth = auth::shared_auth(&[], false); // auth off so /api/me is 200
        let (console, sink) = Console::test_capture(false);
        let params = RouterParams::plain_http().with_console(console, true);
        let app = build_app(handle, auth, Router::new(), params).0;

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
        let auth = auth::shared_auth(&[], false);
        let (console, sink) = Console::test_capture(false);
        // access_log = false.
        let params = RouterParams::plain_http().with_console(console, false);
        let app = build_app(handle, auth, Router::new(), params).0;

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

    /// A no-op console (the flip default) emits nothing even with `access_log`
    /// nominally on — the middleware's `console.is_active()` gate short-circuits.
    /// This is the flip zero-stdout regression guard at the middleware layer.
    #[tokio::test]
    async fn access_log_noop_console_emits_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test_engine_handle(tmp.path());
        let auth = auth::shared_auth(&[], false);
        // The default plain_http params carry a no-op console; force access_log on
        // to prove the console-activity gate (not just the toggle) suppresses it.
        let params = RouterParams {
            console: Console::noop(),
            access_log: true,
            ..RouterParams::plain_http()
        };
        // The router still builds; nothing should panic and nothing is observable
        // (a no-op console drops every line). We assert the request succeeds.
        let app = build_app(handle, auth, Router::new(), params).0;
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
            r#"{"event":"status","key":"pull","tone":"busy","message":"Pulling…","scope":"all"}"#
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
            },
            KeyedWireStatus {
                key: Some("acme".into()),
                tone: "info".into(),
                message: "Certificate renewed.".into(),
                scope: StatusScope::All,
            },
            KeyedWireStatus {
                key: None,
                tone: "warning".into(),
                message: "Worktree dirty.".into(),
                scope: StatusScope::All,
            },
        ];
        let events = status_events(&snapshot, "conn");
        assert_eq!(events.len(), 3, "one event per open status entry");
        let keys: Vec<Option<&str>> = events.iter().map(|e| e.key.as_deref()).collect();
        assert_eq!(keys, vec![Some("pull"), Some("acme"), None]);
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
            },
            KeyedWireStatus {
                key: Some("other".into()),
                tone: "busy".into(),
                message: "Working\u{2026}".into(),
                scope: StatusScope::All,
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
        };
        assert_eq!(
            serde_json::to_string(&ev).unwrap(),
            r#"{"event":"connected","id":"abc-123"}"#
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
            },
            KeyedWireStatus {
                key: Some("acme".into()),
                tone: "info".into(),
                message: "Certificate renewed.".into(),
                scope: StatusScope::All,
            },
        ];
        // Connection B joins: it sees only the `All` status, not A's busy.
        let events = status_events(&snapshot, "B");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key.as_deref(), Some("acme"));
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
        let auth = auth::shared_auth(&[], false);
        let app = build_app(handle, auth, Router::new(), RouterParams::plain_http()).0;

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
            "palette_commands",
            "welcome_tips",
            "dux_version",
            "randomize_agent_names_by_default",
            "gh_available",
            "pr_banner_position",
            "agent_scrollback_lines",
            "show_changes_pane",
            "global_env",
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
}
