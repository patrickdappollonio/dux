//! Web-surface authentication: the session-backed login gate.
//!
//! ## Why not `axum-login`?
//!
//! The locked design names `axum-login` and `tower-sessions` as candidates. For
//! a SINGLE credential backend (bcrypt against config-loaded users) and three
//! routes (`/api/login`, `/api/logout`, `/api/me`), `axum-login` does not pay
//! its way: it would have us implement its `AuthnBackend` trait plus carry an
//! `AuthSession` extractor and a user-store abstraction that all ultimately wrap
//! `tower-sessions` anyway. We get a smaller, more legible surface by using
//! `tower-sessions` directly with a small login handler — so that is what this
//! module does. `tower-sessions` is the floor (no hand-rolled session crypto).
//!
//! ## State plumbing and live reload
//!
//! Credentials are parsed from config ONCE at startup into [`AuthState`] (an
//! A1-review obligation: parsing per request would amplify malformed-entry logs
//! and burn CPU). The parsed state lives behind a shared `Arc<RwLock<AuthState>>`
//! ([`SharedAuth`]) so the engine actor can REBUILD it when a config reload lands
//! (see `engine_actor::run_engine_loop`), letting `reload-config` pick up user
//! changes without a server restart. The login handler reads a cheap clone of the
//! current state under a brief read lock — never holding the guard across an
//! `.await`.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::server::AppState;

/// Session key holding the logged-in username. Its presence is the "logged in"
/// marker — there is no separate boolean, so the session can never be in the
/// inconsistent "marked logged in but no user" state.
pub(crate) const SESSION_USER_KEY: &str = "username";

/// Cookie name for the session. A product-specific name (rather than the
/// `tower-sessions` default `"id"`) avoids collisions when several apps share a
/// host during development.
pub(crate) const SESSION_COOKIE_NAME: &str = "dux_session";

/// Inactivity window after which an idle session expires and the user must log
/// in again. Seven days balances "don't nag an active operator" against "a
/// forgotten open tab shouldn't stay authenticated forever". Sessions also die
/// with the server (the `MemoryStore` is in-memory) — a restart forces re-login.
pub(crate) const SESSION_INACTIVITY_DAYS: i64 = 7;

/// A single parsed credential, OWNED (unlike `dux_core::auth::ParsedUser`, which
/// borrows from the config strings). We re-serialize to the `"name:hash"` shape
/// `dux_core::auth::verify_credentials` expects, so the one bcrypt-verify-per-call
/// implementation (with its timing mitigation) stays the single source of truth.
#[derive(Clone, Debug)]
pub(crate) struct OwnedUser {
    pub(crate) username: String,
    pub(crate) hash: String,
}

/// The login gate's parsed, owned credential snapshot.
///
/// Built ONCE at startup from the config users plus the `--disable-auth` flag,
/// then rebuilt in place on config reload. `enabled` mirrors
/// `dux_core::auth::auth_enabled`: the gate is on only when auth is not disabled
/// AND at least one entry parsed valid.
#[derive(Clone, Debug, Default)]
pub struct AuthState {
    pub enabled: bool,
    pub(crate) users: Vec<OwnedUser>,
}

impl AuthState {
    /// Build the auth snapshot from raw config `users` entries and the
    /// `--disable-auth` flag, emitting the entries-present-but-none-valid startup
    /// warning (A1-review obligation) when applicable.
    ///
    /// The warning fires when the operator clearly INTENDED auth (they wrote
    /// `[auth]` entries) but every entry failed to parse, so the gate silently
    /// stays OFF — a loopback fail-open footgun. We surface it on BOTH the logger
    /// (dux.log) and stderr because an operator launching `dux server` watches the
    /// terminal, while a longer-running server is diagnosed from the log.
    pub(crate) fn build(users: &[String], disable_auth: bool) -> Self {
        let parsed = dux_core::auth::parse_users(users);
        let owned: Vec<OwnedUser> = parsed
            .iter()
            .map(|u| OwnedUser {
                username: u.username.to_string(),
                hash: u.hash.to_string(),
            })
            .collect();

        if !disable_auth && !users.is_empty() && owned.is_empty() {
            let warning = "auth is OFF despite [auth] entries — every entry failed to parse. \
                 Fix the \"username:bcrypt-hash\" lines in config.toml (or pass --disable-auth \
                 to make running without a login gate explicit).";
            dux_core::logger::warn(warning);
            eprintln!("WARNING: {warning}");
        }

        let enabled = !disable_auth && !owned.is_empty();
        AuthState {
            enabled,
            users: owned,
        }
    }

    /// Rebuild the snapshot on a config reload.
    ///
    /// `prev` is the gate snapshot BEFORE this rebuild. `host_only` is the
    /// DOWNGRADE-RULE classification — NOT the same as the startup bind gate's
    /// "local" classification. It is `true` ONLY when EVERY live listener is a
    /// genuine loopback address (`127.0.0.1`/`::1`). A Tailscale bind makes the
    /// server reachable from other people's devices on a shared tailnet, so even
    /// though the startup bind gate deliberately treats Tailscale as "local"
    /// (deciding whether a public bind needs `--insecure-allow-remote`), it is
    /// NOT host-only for the downgrade rule: a tailnet-reachable server must
    /// never silently drop its login gate on reload. Callers compute this from
    /// their actual listeners (loopback only → `true`; any Tailscale or public
    /// listener → `false`), threaded from the server entry point.
    ///
    /// Removing the last user (or otherwise clearing the credentials) flips
    /// `enabled` false, but the bind gate only protects at STARTUP — a running
    /// server stays bound, so a previously protected reachable server would
    /// silently become open. How we handle that
    /// enabled→disabled-because-users-dropped-to-zero transition depends on
    /// `host_only`:
    ///
    /// - **Not host-only (reachable bind — public OR Tailscale): REFUSE the
    ///   downgrade.** The gate never downgrades to open on an address other
    ///   people can reach. We KEEP the prior snapshot (users + enabled stay) and
    ///   log a loud error. Restarting the server is the explicit way to run open
    ///   here — the startup bind gate then demands `--insecure-allow-remote`.
    /// - **Host-only bind (loopback only): allow the downgrade with a warning.**
    ///   A loopback-only server is reachable from the same host alone, so
    ///   flipping the gate off is the documented behavior; we still warn so the
    ///   operator is not unaware.
    ///
    /// The "users dropped to zero" rule covers the entries-present-but-all-
    /// malformed case too: `parse_users` returns empty there, so `build` reports
    /// `enabled = false` and this transition fires.
    ///
    /// NOTE: the guard keys on `next.users.is_empty()`, so it only catches an
    /// enabled→disabled transition caused by the users going away. A reload that
    /// turned the gate off while users *remain* (i.e. `disable_auth` flipping
    /// true) would NOT be refused. That is safe only because `disable_auth` is a
    /// process-lifetime CLI flag that cannot change at reload time; if it ever
    /// becomes reloadable, this guard must be revisited.
    ///
    /// Any other transition (enabling from disabled, swapping which users exist
    /// while at least one remains, a no-op reload) simply takes the rebuilt
    /// snapshot.
    ///
    /// Returns the (possibly-kept) snapshot plus a `refused` flag: `true` only
    /// when a non-host-only downgrade was REFUSED (the `[auth]` change was NOT
    /// applied). The caller uses it to report a warn-tone reload status instead
    /// of a plain success — otherwise the refusal's "why" lives only in the log.
    pub(crate) fn rebuild(
        prev: &AuthState,
        users: &[String],
        disable_auth: bool,
        host_only: bool,
    ) -> (Self, bool) {
        let next = AuthState::build(users, disable_auth);
        let downgrades_to_open = prev.enabled && !next.enabled && next.users.is_empty();
        if downgrades_to_open && !host_only {
            // REFUSE: the gate must not silently open a server other people can
            // reach (a public bind OR a Tailscale bind — a shared tailnet means
            // other devices, not just this host). Keep the prior snapshot (users
            // + enabled) so the live server stays protected; a restart is the
            // explicit, gated way to run open.
            let error = "[auth] reload would remove the last user and turn the login gate OFF, \
                but this server is reachable from other devices (it is bound to a NON-LOOPBACK \
                address — a public address or your Tailscale IP, which is reachable by everyone \
                on that tailnet). Refusing the downgrade: the previous users are kept and the \
                gate stays ON. To run with no login on a reachable address, restart the server \
                (the startup bind gate then requires --insecure-allow-remote).";
            dux_core::logger::error(error);
            eprintln!("ERROR: {error}");
            return (prev.clone(), true);
        }
        if downgrades_to_open {
            // Host-only (loopback only): the documented downgrade is allowed;
            // warn so the operator is aware the gate is now off.
            let warning = "[auth] users removed — the login gate is now OFF. This server is \
                bound to loopback only, so it is reachable from this host alone, but anyone \
                with local access can now control your agents and worktrees. Add a user to \
                re-protect it.";
            dux_core::logger::warn(warning);
            eprintln!("WARNING: {warning}");
        }
        (next, false)
    }

    /// Re-serialize the owned users to the `"name:hash"` shape that
    /// `dux_core::auth::verify_credentials` consumes, so verification (one bcrypt
    /// op per call, with the unknown-user timing mitigation) stays in core.
    fn as_config_entries(&self) -> Vec<String> {
        self.users
            .iter()
            .map(|u| format!("{}:{}", u.username, u.hash))
            .collect()
    }
}

/// Shared, swappable auth snapshot. Read by the login handler and the WS gate;
/// rebuilt by the engine actor on config reload. A `std::sync::RwLock` is enough:
/// reads are brief (clone the small state out, drop the guard before any await),
/// and the only writer is the engine loop applying a reload.
pub type SharedAuth = Arc<RwLock<AuthState>>;

/// Build a [`SharedAuth`] from config users and the disable flag.
pub fn shared_auth(users: &[String], disable_auth: bool) -> SharedAuth {
    Arc::new(RwLock::new(AuthState::build(users, disable_auth)))
}

/// Whether the gate is currently enabled (brief read lock).
pub(crate) fn is_enabled(auth: &SharedAuth) -> bool {
    auth.read().map(|s| s.enabled).unwrap_or(false)
}

/// Whether `username` is still a configured user in the CURRENT auth snapshot
/// (brief read lock; a `Vec` scan, NO bcrypt). Used by the gate and `/api/me` to
/// re-verify a live session on every request so removing a user (config edit or
/// TUI palette + `reload-config`) immediately revokes that user's session —
/// presence of a session cookie is not enough; the user must still exist.
///
/// Returns `false` when the lock is poisoned: a poisoned auth snapshot can't be
/// trusted to authorize, so we fail closed.
pub(crate) fn username_exists(auth: &SharedAuth, username: &str) -> bool {
    auth.read()
        .map(|s| s.users.iter().any(|u| u.username == username))
        .unwrap_or(false)
}

/// Read the session's username and return it only if it STILL names a configured
/// user in the current auth snapshot. Returns `None` for a missing session, a
/// session whose user has been removed (config edit or TUI palette +
/// `reload-config`), or a session error — flushing the now-orphaned session
/// internally so its cookie can't keep retrying. Shared by the HTTP gate, the
/// `/api/me` handler, and the live WebSocket re-verify so all three apply the
/// same "presence of a cookie is not enough; the user must still exist" rule.
pub(crate) async fn session_user_if_valid(auth: &SharedAuth, session: &Session) -> Option<String> {
    match session.get::<String>(SESSION_USER_KEY).await {
        Ok(Some(username)) if username_exists(auth, &username) => Some(username),
        Ok(Some(_)) => {
            // The session names a user who no longer exists (removed + reloaded):
            // destroy the orphaned session so its cookie can't keep retrying.
            let _ = session.flush().await;
            None
        }
        Ok(None) => None,
        Err(_) => {
            // A corrupted or unreadable session record: flush it too, so its
            // cookie can't keep presenting the same bad state on every request.
            let _ = session.flush().await;
            None
        }
    }
}

// --- Per-IP login attempt backoff -----------------------------------------

/// Failed logins allowed per IP within [`RATE_WINDOW`] before the IP is told to
/// back off with `429`.
pub(crate) const RATE_LIMIT_MAX_FAILURES: u32 = 5;

/// Sliding window for the per-IP failure counter.
pub(crate) const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Hard cap on the number of per-IP buckets retained at once. `/api/login` is
/// pre-auth reachable, so an attacker controlling an IPv6 `/64` (or any large
/// address pool) can rotate source addresses and mint a fresh bucket per failed
/// attempt — unbounded memory growth on exactly the non-loopback deployments
/// auth enables. We bound the map: 4096 distinct active attackers is far beyond
/// any legitimate concurrent-login load, and even a full map is a few hundred KB.
/// When the cap is reached and a NEW IP must be inserted, the STALEST bucket
/// (oldest `window_start`) is evicted. Best-effort by design: under a flood the
/// evicted IPs simply lose their accumulated count and start fresh, which only
/// ever loosens throttling (never blocks a legitimate user), and the per-IP
/// bcrypt cost still rate-limits each individual attempt.
pub(crate) const RATE_LIMIT_MAX_BUCKETS: usize = 4096;

/// Coarse, best-effort, MEMORY-ONLY per-IP login backoff.
///
/// This is deliberately simple: a map of `IP -> (failure count, window start)`.
/// After [`RATE_LIMIT_MAX_FAILURES`] failed attempts inside [`RATE_WINDOW`] the
/// IP receives `429 Too Many Requests` with `Retry-After` until the window
/// rolls over. A SUCCESSFUL login clears the IP's counter, so a legitimate user
/// who eventually types the right password is never left throttled.
///
/// ## Bounding the map (best-effort)
///
/// The map is kept bounded so a pre-auth caller rotating source addresses can't
/// grow it without limit (an IPv6 `/64` is 2^64 addresses):
/// - **Opportunistic eviction:** whenever a request inserts a NEW ip,
///   `check_and_charge` first sweeps out every bucket whose window has fully
///   elapsed. The sweep is O(n) but only runs on the rarer new-ip insert path
///   (a repeat offender's bucket already exists, so it skips the sweep), making
///   it amortized-rare under normal traffic and self-limiting under a flood
///   (each unique source pays one sweep, reclaiming all the previous floods'
///   expired entries).
/// - **Hard cap:** if the map is still at [`RATE_LIMIT_MAX_BUCKETS`] after the
///   sweep (i.e. everything is within-window), the stalest bucket (oldest
///   `window_start`) is dropped to make room. Evicting the stalest entry costs
///   the attacker the least and protects the freshest live throttles.
///
/// Limitations (documented, for the council to weigh): it is per-process (a
/// restart resets it), it keys on the peer address only (a NAT or proxy shares
/// one bucket; behind a reverse proxy the peer is the proxy — `X-Forwarded-For`
/// is intentionally NOT trusted here), and it is not a substitute for an
/// upstream WAF. It exists to blunt trivial online password guessing, not to be
/// a complete anti-bruteforce system. The window/limit/cap are
/// constructor-injectable so tests can drive the thresholds deterministically
/// without sleeping.
#[derive(Clone)]
pub struct RateLimiter {
    max_failures: u32,
    window: Duration,
    max_buckets: usize,
    buckets: Arc<RwLock<HashMap<IpAddr, Bucket>>>,
}

#[derive(Clone, Copy)]
struct Bucket {
    failures: u32,
    window_start: Instant,
}

impl RateLimiter {
    pub(crate) fn new(max_failures: u32, window: Duration) -> Self {
        Self::with_cap(max_failures, window, RATE_LIMIT_MAX_BUCKETS)
    }

    /// Like [`RateLimiter::new`] but with an injectable bucket cap so tests can
    /// exercise the eviction path with a tiny map instead of allocating 4096
    /// entries.
    pub(crate) fn with_cap(max_failures: u32, window: Duration, max_buckets: usize) -> Self {
        Self {
            // Clamp to at least 1: max_failures = 0 would make the budget check
            // `bucket.failures >= self.max_failures` (i.e. `0 >= 0`) true on the
            // very first attempt and lock every IP out of login entirely.
            max_failures: max_failures.max(1),
            window,
            max_buckets: max_buckets.max(1),
            buckets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Atomically check the per-IP failure budget AND charge this attempt against
    /// it under a single write lock. This closes the check-then-act race: when
    /// several logins from one IP arrive at once, each charges here BEFORE the
    /// slow, lock-free bcrypt verify runs, so the budget is consumed atomically
    /// instead of every concurrent attempt reading the same pre-charge count and
    /// slipping through.
    ///
    /// Returns `Some(retry_after_secs)` when the IP is already over budget — the
    /// attempt is refused with `429` and is NOT charged further. Returns `None`
    /// when the attempt is allowed; on the normal path it has now been charged
    /// (one failure unit), but on the fail-open poison path below it is allowed
    /// WITHOUT a charge. Any outcome that is NOT a confirmed wrong password (a
    /// success, an infra error, or a session error) is refunded by the login
    /// handler with a single [`RateLimiter::clear`] call — placed before the
    /// session commit so every error return after a correct verify is already
    /// covered — which resets the IP's whole bucket so a legitimate user is never
    /// left throttled; only a confirmed wrong password leaves the charge in place,
    /// so the retained count reflects failures.
    ///
    /// On the NEW-ip path (the only path that can grow the map) it first sweeps
    /// expired buckets, then enforces the hard cap by evicting the stalest entry
    /// if still full — see [`RateLimiter`] for the bounding rationale. A repeat
    /// offender's bucket already exists, so the common hot path skips both.
    fn check_and_charge(&self, ip: IpAddr) -> Option<u64> {
        let Ok(mut buckets) = self.buckets.write() else {
            // Deliberate fail-OPEN: a poisoned limiter must not lock out
            // legitimate users (the asymmetry vs `username_exists`, which fails
            // closed). Allowing the attempt only loosens throttling, and the
            // per-attempt bcrypt cost still rate-limits each guess.
            return None;
        };
        let now = Instant::now();

        // Bound the map only when this attempt would insert a NEW ip: a repeat
        // offender reuses its bucket and never grows the map, so it skips the
        // O(n) work entirely.
        if !buckets.contains_key(&ip) {
            // Opportunistic eviction: drop every bucket whose window has fully
            // elapsed (those are dead weight — they would reset on next sight
            // anyway).
            buckets.retain(|_, b| b.window_start.elapsed() < self.window);

            // Hard cap: if every surviving bucket is still within its window,
            // evict the stalest (oldest `window_start`) to make room.
            if buckets.len() >= self.max_buckets
                && let Some(stalest) = buckets
                    .iter()
                    .min_by_key(|(_, b)| b.window_start)
                    .map(|(ip, _)| *ip)
            {
                buckets.remove(&stalest);
            }
        }

        let bucket = buckets.entry(ip).or_insert(Bucket {
            failures: 0,
            window_start: now,
        });
        if bucket.window_start.elapsed() >= self.window {
            // Window rolled over: start a fresh budget for this IP.
            bucket.failures = 0;
            bucket.window_start = now;
        }
        if bucket.failures >= self.max_failures {
            // Already over budget: refuse this attempt without charging further.
            let remaining = self.window.saturating_sub(bucket.window_start.elapsed());
            return Some(remaining.as_secs().max(1));
        }
        // Charge this in-flight attempt against the budget BEFORE the verify, so
        // concurrent attempts can't all slip through on a stale count.
        bucket.failures = bucket.failures.saturating_add(1);
        None
    }

    /// Current number of retained buckets. Test-only visibility into the map
    /// size so the eviction/cap tests can assert it shrinks/stays bounded.
    #[cfg(test)]
    fn bucket_count(&self) -> usize {
        self.buckets.read().map(|b| b.len()).unwrap_or(0)
    }

    /// Clear an IP's counter after a successful login so a legitimate user is
    /// never throttled once they get in.
    fn clear(&self, ip: IpAddr) {
        if let Ok(mut buckets) = self.buckets.write() {
            buckets.remove(&ip);
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RATE_LIMIT_MAX_FAILURES, RATE_WINDOW)
    }
}

// --- Login / logout / me handlers -----------------------------------------

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    username: String,
}

/// Generic, user-enumeration-safe failure body. We never reveal whether the
/// username exists — the same message covers "no such user" and "wrong password".
const LOGIN_FAILED_MESSAGE: &str = "Invalid username or password.";

/// `POST /api/login` — verify credentials, mint a session, rotate the session id.
///
/// When auth is OFF the endpoint is a no-op success (the SPA never shows a login
/// form in that mode, but a stray POST shouldn't 500). On success we
/// `cycle_id()` BEFORE inserting the username so the post-login session id is
/// fresh — anti-session-fixation: an id an attacker may have planted pre-login is
/// discarded. Failures increment the per-IP backoff and return a generic `401`.
pub(crate) async fn login(
    State(state): State<AppState>,
    session: Session,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginRequest>,
) -> axum::response::Response {
    // Snapshot the auth state under a brief read lock; never hold the guard
    // across the awaits below.
    let (enabled, entries) = {
        let guard = match state.auth.read() {
            Ok(guard) => guard,
            Err(_) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "auth state poisoned").into_response();
            }
        };
        (guard.enabled, guard.as_config_entries())
    };

    if !enabled {
        // Auth disabled: nothing to verify. Report success without a session so
        // the SPA's optimistic flows don't error.
        return (
            StatusCode::OK,
            Json(LoginResponse {
                username: body.username,
            }),
        )
            .into_response();
    }

    let ip = peer.ip();

    // Atomic check-and-charge BEFORE the (expensive) bcrypt verify: a throttled
    // IP can't keep us burning CPU, AND charging the attempt under the same lock
    // closes the check-then-act race where concurrent attempts from one IP would
    // all pass a separate check before any recorded a failure.
    if let Some(retry_after) = state.rate_limiter.check_and_charge(ip) {
        // Console: rate-limited attempt (IP only — never the attempted username,
        // per the auth-slice log-hygiene rule).
        state.console.login_rate_limited(ip);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", retry_after.to_string())],
            "Too many failed login attempts. Try again later.",
        )
            .into_response();
    }

    // bcrypt verify is a deliberately slow (~250ms at cost 12) CPU-bound op, so
    // run it off the async reactor in spawn_blocking — the same discipline every
    // other blocking op in this crate follows (see the spawn_blocking sites in
    // server.rs). The entries were already snapshotted under the brief read lock
    // above; move the credential clones into the blocking task.
    let username = body.username.clone();
    let password = body.password.clone();
    let ok = match tokio::task::spawn_blocking(move || {
        dux_core::auth::verify_credentials(&entries, &username, &password)
    })
    .await
    {
        Ok(ok) => ok,
        Err(e) => {
            // Infrastructure failure (the bcrypt task panicked or the runtime is
            // winding down), NOT a wrong password — refund the attempt charged by
            // check_and_charge so a transient error can't burn a legit user's budget.
            state.rate_limiter.clear(ip);
            dux_core::logger::error(&format!("login verify task failed: {e}"));
            return (StatusCode::INTERNAL_SERVER_ERROR, "verify error").into_response();
        }
    };
    if !ok {
        // A CONFIRMED wrong password — the ONLY outcome that keeps the charge
        // check_and_charge placed against the budget above.
        // Console: failed login (IP ONLY — never the attempted username, per the
        // auth-slice log-hygiene rule: a username in the log is an enumeration
        // leak).
        state.console.login_failed(ip);
        return (StatusCode::UNAUTHORIZED, LOGIN_FAILED_MESSAGE).into_response();
    }

    // The password was correct, so this attempt is not a failure. Refund the
    // pre-charged attempt NOW — before the session commit below — so a session
    // error on a correct-password login cannot leave the attempt counted against
    // the IP's budget and eventually 429 a legitimate user.
    state.rate_limiter.clear(ip);

    // Anti-fixation: rotate the id BEFORE associating the user with the session.
    if let Err(e) = session.cycle_id().await {
        dux_core::logger::error(&format!("failed to rotate session id on login: {e}"));
        return (StatusCode::INTERNAL_SERVER_ERROR, "session error").into_response();
    }
    if let Err(e) = session.insert(SESSION_USER_KEY, &body.username).await {
        dux_core::logger::error(&format!("failed to persist session on login: {e}"));
        return (StatusCode::INTERNAL_SERVER_ERROR, "session error").into_response();
    }

    // Console: successful login. The username IS logged on success (the operator
    // wants to know who got in — this is not an enumeration leak).
    state.console.login_ok(&body.username, ip);

    (
        StatusCode::OK,
        Json(LoginResponse {
            username: body.username,
        }),
    )
        .into_response()
}

/// `POST /api/logout` — destroy the session. Idempotent: logging out when not
/// logged in (or with auth off) still returns `204`, so the endpoint can live
/// outside the gate.
pub(crate) async fn logout(
    State(state): State<AppState>,
    session: Session,
) -> axum::response::Response {
    // Read the username BEFORE flushing so the console can name who logged out
    // (the session was already authenticated, so this is not an enumeration
    // leak). `None` for an unauthenticated/auth-off logout — nothing to announce.
    let username = session.get::<String>(SESSION_USER_KEY).await.ok().flatten();
    if let Err(e) = session.flush().await {
        dux_core::logger::warn(&format!("failed to flush session on logout: {e}"));
    }
    if let Some(username) = username {
        state.console.logout(&username);
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Serialize)]
#[serde(untagged)]
enum MeResponse {
    /// Auth is configured off entirely; the SPA skips the login screen.
    Disabled { auth: &'static str },
    /// Auth is on and the request carries a valid session.
    Authed { username: String },
}

/// `GET /api/me` — report the caller's auth state so the SPA can decide whether
/// to render the login screen, the app shell, or skip auth entirely.
///
/// Three outcomes the SPA must distinguish:
/// - auth OFF        → `200 {"auth":"disabled"}`
/// - auth ON, session→ `200 {"username":"..."}`
/// - auth ON, none   → `401`
///
/// Mirrors the gate's user-existence re-check: a session whose username has been
/// removed from the current snapshot reports `401` (and its orphaned session is
/// flushed), not the stale username — so the SPA falls back to the login screen
/// the moment the operator revokes the user, matching the gate's behavior.
pub(crate) async fn me(
    State(state): State<AppState>,
    session: Session,
) -> axum::response::Response {
    if !is_enabled(&state.auth) {
        return (
            StatusCode::OK,
            Json(MeResponse::Disabled { auth: "disabled" }),
        )
            .into_response();
    }
    match session_user_if_valid(&state.auth, &session).await {
        Some(username) => (StatusCode::OK, Json(MeResponse::Authed { username })).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn build_disabled_when_no_users() {
        let state = AuthState::build(&[], false);
        assert!(!state.enabled);
        assert!(state.users.is_empty());
    }

    #[test]
    fn build_enabled_with_one_valid_user() {
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let state = AuthState::build(&[format!("alice:{hash}")], false);
        assert!(state.enabled);
        assert_eq!(state.users.len(), 1);
        assert_eq!(state.users[0].username, "alice");
    }

    #[test]
    fn build_disabled_when_flag_set_even_with_users() {
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let state = AuthState::build(&[format!("alice:{hash}")], true);
        assert!(!state.enabled);
        // Users are still parsed (so a later reload that clears the flag works),
        // but the gate is off.
        assert_eq!(state.users.len(), 1);
    }

    #[test]
    fn build_disabled_when_all_entries_malformed() {
        // Entries present but none valid: the gate stays OFF. (The startup warning
        // is emitted as a side effect; here we assert the effective state, which
        // is the A1 obligation's minimum bar.)
        let state = AuthState::build(&["garbage".to_string(), ":nohash".to_string()], false);
        assert!(!state.enabled);
        assert!(state.users.is_empty());
    }

    #[test]
    fn as_config_entries_round_trips_through_core_verify() {
        let hash = dux_core::auth::hash_password("hunter2").unwrap();
        let state = AuthState::build(&[format!("alice:{hash}")], false);
        let entries = state.as_config_entries();
        assert!(dux_core::auth::verify_credentials(
            &entries, "alice", "hunter2"
        ));
        assert!(!dux_core::auth::verify_credentials(
            &entries, "alice", "wrong"
        ));
    }

    /// Build an enabled snapshot with a single valid user, for use as the `prev`
    /// argument to `rebuild` in the transition tests below.
    fn enabled_with_user(name: &str) -> AuthState {
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let prev = AuthState::build(&[format!("{name}:{hash}")], false);
        assert!(prev.enabled, "fixture must start enabled");
        prev
    }

    #[test]
    fn rebuild_host_only_disables_when_last_user_removed() {
        // HOST-ONLY (loopback-only) bind: enabled → disabled (the last user
        // removed) is ALLOWED, with a warning side effect. Here we assert the
        // effective transition.
        let prev = enabled_with_user("alice");
        let (next, refused) = AuthState::rebuild(&prev, &[], false, true);
        assert!(
            !next.enabled,
            "on a host-only bind, removing the last user must turn the gate off"
        );
        assert!(next.users.is_empty());
        assert!(
            !refused,
            "a host-only downgrade is allowed, not refused — refused must be false"
        );
    }

    #[test]
    fn rebuild_non_host_only_refuses_last_user_removal() {
        // NOT host-only (a reachable bind): the same enabled → disabled downgrade
        // is REFUSED. The prior snapshot is kept (users + enabled stay) so the
        // live server never silently opens; a restart is the explicit way to run
        // open.
        let prev = enabled_with_user("alice");
        let (next, refused) = AuthState::rebuild(&prev, &[], false, false);
        assert!(
            next.enabled,
            "on a reachable bind, the gate must NOT downgrade to open"
        );
        assert_eq!(next.users.len(), 1, "the previous user must be kept");
        assert_eq!(next.users[0].username, "alice");
        assert!(
            refused,
            "a refused non-host-only downgrade must signal refused = true"
        );
    }

    #[test]
    fn rebuild_tailscale_bind_refuses_last_user_removal() {
        // A Tailscale-bound server is NOT host-only: a shared tailnet means other
        // people's devices can reach it, so the downgrade must be REFUSED exactly
        // like any other reachable bind. This is the F1 regression guard — the
        // startup bind gate treats Tailscale as "local", but the DOWNGRADE rule
        // must not, so the caller passes host_only = false for a Tailscale bind.
        let prev = enabled_with_user("alice");
        let (next, refused) = AuthState::rebuild(&prev, &[], false, false);
        assert!(
            next.enabled,
            "a Tailscale-bound server must NOT downgrade its login gate to open"
        );
        assert_eq!(next.users.len(), 1, "the previous user must be kept");
        assert!(
            refused,
            "a Tailscale downgrade must be refused (host_only = false)"
        );
    }

    #[test]
    fn rebuild_non_host_only_refuses_when_entries_all_malformed() {
        // Entries present but none valid counts as "users dropped to zero": on a
        // reachable bind the downgrade is still REFUSED.
        let prev = enabled_with_user("alice");
        let (next, refused) = AuthState::rebuild(
            &prev,
            &["garbage".to_string(), ":nohash".to_string()],
            false,
            false,
        );
        assert!(
            next.enabled,
            "all-malformed entries must not open a reachable server"
        );
        assert_eq!(next.users.len(), 1, "the previous valid user must be kept");
        assert!(
            refused,
            "an all-malformed non-host-only downgrade must also signal refused = true"
        );
    }

    #[test]
    fn rebuild_non_host_only_allows_swapping_users() {
        // A reload that REPLACES the user (still one valid user remaining) is not
        // a downgrade-to-open, so even a reachable bind takes the new snapshot.
        let prev = enabled_with_user("alice");
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let (next, refused) = AuthState::rebuild(&prev, &[format!("bob:{hash}")], false, false);
        assert!(next.enabled, "a remaining valid user keeps the gate on");
        assert_eq!(next.users.len(), 1);
        assert_eq!(next.users[0].username, "bob");
        assert!(
            !refused,
            "swapping users (one still valid) is not a downgrade, so not refused"
        );
    }

    #[test]
    fn rebuild_stays_enabled_when_a_user_remains() {
        let prev = enabled_with_user("alice");
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let (next, refused) = AuthState::rebuild(&prev, &[format!("bob:{hash}")], false, true);
        assert!(next.enabled, "a remaining valid user keeps the gate on");
        assert_eq!(next.users.len(), 1);
        assert!(!refused, "a user remains, so nothing was refused");
    }

    #[test]
    fn rebuild_enabling_from_disabled_does_not_trip_the_warning_path() {
        // disabled → enabled (a first user added): no enabled→disabled
        // transition, so the gate simply comes on (loopback-ness is irrelevant).
        let prev = AuthState::build(&[], false);
        assert!(!prev.enabled);
        let hash = dux_core::auth::hash_password("pw").unwrap();
        let (next, refused) = AuthState::rebuild(&prev, &[format!("alice:{hash}")], false, false);
        assert!(next.enabled);
        assert_eq!(next.users.len(), 1);
        assert!(!refused, "enabling from disabled is never a refusal");
    }

    #[test]
    fn rate_limiter_blocks_after_max_failures_and_resets_on_success() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        let addr = ip("10.0.0.1");

        // Under the budget: each attempt is allowed and charges itself.
        for _ in 0..3 {
            assert!(limiter.check_and_charge(addr).is_none());
        }
        // Now over the budget: blocked with a positive retry-after (a blocked
        // attempt does not charge further).
        let retry = limiter.check_and_charge(addr).expect("should be throttled");
        assert!(retry >= 1);

        // A success clears the bucket, so the IP can try again immediately.
        limiter.clear(addr);
        assert!(limiter.check_and_charge(addr).is_none());
    }

    #[test]
    fn rate_limiter_window_rollover_resets_an_over_budget_ip() {
        // Saturate the budget so the IP is throttled, THEN let the window elapse
        // and prove the next attempt is allowed again — i.e. the rollover branch
        // resets an OVER-budget bucket, not merely an under-budget one (a
        // regression that only reset under-budget buckets would leave a throttled
        // IP blocked forever).
        let window = Duration::from_millis(20);
        let limiter = RateLimiter::new(1, window);
        let addr = ip("10.0.0.2");
        // First attempt is allowed (charges to the max of 1)...
        assert!(limiter.check_and_charge(addr).is_none());
        // ...the second is over budget and throttled.
        assert!(
            limiter.check_and_charge(addr).is_some(),
            "the IP must be throttled once it is over budget"
        );
        // After the window fully elapses, the rollover must reset the over-budget
        // count and allow attempts again.
        std::thread::sleep(window + Duration::from_millis(20));
        assert!(
            limiter.check_and_charge(addr).is_none(),
            "an elapsed window must reset an over-budget counter and allow attempts"
        );
    }

    #[test]
    fn rate_limiter_keys_per_ip() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        let a = ip("10.0.0.3");
        let b = ip("10.0.0.4");
        assert!(
            limiter.check_and_charge(a).is_none(),
            "a's one attempt charges"
        );
        assert!(limiter.check_and_charge(a).is_some(), "a is now throttled");
        assert!(
            limiter.check_and_charge(b).is_none(),
            "b has its own bucket and is unaffected"
        );
    }

    #[test]
    fn rate_limiter_evicts_stale_buckets_on_new_ip_insert() {
        // A zero-length window means every existing bucket reads as fully
        // elapsed, so the opportunistic sweep on the next new-ip insert must
        // reclaim them — the map should not grow unboundedly with expired
        // entries. We use a generous cap so the SWEEP (not the hard cap) is what
        // shrinks the map.
        let limiter = RateLimiter::with_cap(5, Duration::from_millis(0), 4096);

        // Seed several distinct IPs; each is immediately stale (window is 0).
        for i in 0..10u8 {
            let _ = limiter.check_and_charge(ip(&format!("10.1.0.{i}")));
        }
        // Each new-ip insert swept the prior stale entries first, so the map
        // never accumulates: after the last insert only that one bucket remains.
        assert_eq!(
            limiter.bucket_count(),
            1,
            "stale buckets must be swept when a new ip is inserted"
        );

        // Inserting one more new ip proves the shrink again (sweep leaves only
        // the freshly inserted bucket).
        let _ = limiter.check_and_charge(ip("10.1.0.250"));
        assert_eq!(limiter.bucket_count(), 1, "the sweep keeps the map bounded");
    }

    #[test]
    fn rate_limiter_enforces_hard_cap_when_all_buckets_live() {
        // A long window means nothing expires, so the SWEEP can't help — the
        // hard cap must kick in. A tiny injectable cap keeps the test fast.
        let cap = 3;
        let limiter = RateLimiter::with_cap(5, Duration::from_secs(3600), cap);

        // Insert more distinct IPs than the cap; all are within-window.
        for i in 0..(cap as u16 + 5) {
            let _ = limiter.check_and_charge(ip(&format!("10.2.{}.{}", i / 256, i % 256)));
        }
        assert_eq!(
            limiter.bucket_count(),
            cap,
            "the map must never exceed the hard cap even when nothing expires"
        );
    }

    #[test]
    fn rate_limiter_check_and_charge_is_atomic_under_concurrency() {
        use std::sync::atomic::{AtomicU32, Ordering};
        // Regression guard for the check-then-charge race: many concurrent
        // attempts from ONE ip must collectively consume the budget exactly once.
        // EXACTLY `max_failures` are allowed; every attempt beyond that is blocked
        // (the assertion below is `== max`). With a non-atomic check-then-record
        // (the old two-method design) all of them would slip through. This is
        // deterministic only because check_and_charge holds the write lock across
        // the whole check+charge body — it guards against a refactor that splits
        // that lock, not a probabilistic race.
        let max = 5u32;
        let limiter = RateLimiter::new(max, Duration::from_secs(60));
        let addr = ip("10.3.0.1");
        let attempts = 64;
        let allowed = Arc::new(AtomicU32::new(0));

        let handles: Vec<_> = (0..attempts)
            .map(|_| {
                let l = limiter.clone();
                let a = allowed.clone();
                std::thread::spawn(move || {
                    if l.check_and_charge(addr).is_none() {
                        a.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            allowed.load(Ordering::SeqCst),
            max,
            "concurrent attempts from one IP must not exceed the failure budget"
        );
    }
}
