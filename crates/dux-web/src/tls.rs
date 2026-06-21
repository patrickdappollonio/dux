//! Built-in TLS via ACME (Let's Encrypt, HTTP-01) plus the deferred-from-auth
//! hardening that only makes sense once dux terminates TLS.
//!
//! ## What this module owns
//!
//! - [`build_acme_state`] — turns the resolved [`AcmePlan`] into a live
//!   `rustls_acme::AcmeState` (normalized domains, `mailto:` contact, a `DirCache`
//!   under a freshly-created `0700` directory that holds the ACME account and
//!   certificate PRIVATE keys, staging vs production from config).
//! - [`spawn_acme_event_task`] — the dedicated task that POLLS the `AcmeState`
//!   stream. rustls-acme is runtime-agnostic and runs NO background work on its
//!   own; acquisition and renewal only progress while something drives the
//!   stream. Every event is logged loudly — an `Ok` at info, an `Err` at error —
//!   and the task screams if the stream ever ends, because a silent stop means
//!   certificates quietly stop renewing.
//! - The `:80` router: the real HTTP-01 challenge tower service (exempt from the
//!   Host allowlist — it is token-keyed and harmless) plus a fallback that 308s
//!   everything else to HTTPS via the pure [`redirect_target`].
//! - The Host allowlist middleware ([`host_allowlist_layer`]) that pins requests
//!   to the configured domains (DNS-rebinding defense — deferral 2).
//! - [`SweepableMemoryStore`] + [`spawn_session_sweep`] — the bounded in-memory
//!   session store (deferral 3): `tower_sessions::MemoryStore` never evicts
//!   expired records, so this is a drop-in store that the periodic sweep can
//!   prune.
//! - [`normalize_domains`] — the ONE shared domain-normalization contract used by
//!   both the allowlist and ACME issuance, exported for the binary's URL display.
//!
//! ## Injectability for tests
//!
//! Real ACME cannot run in tests (it needs a public IP, DNS, and a CA). The
//! acceptor is the injectable seam, and the serve path is genuinely SHARED:
//! production binds by address ([`serve_https_acme`] / [`serve_http_challenge`])
//! and delegates to the `from_tcp` cores ([`serve_https_with_acceptor`] /
//! [`serve_http_challenge_from_tcp`]); the e2e pre-binds an ephemeral
//! `127.0.0.1:0` listener and calls those SAME cores directly with a self-signed
//! `RustlsAcceptor` (production injects rustls-acme's `AxumAcceptor`). The only
//! prod-vs-test difference left is the acceptor object and who binds, so the e2e
//! exercises the real connect-info, graceful-shutdown, and WS-over-TLS chain
//! rather than an inline copy of it. See `tests/tls_serving.rs`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use dux_core::wire::WireStatus;
use futures_util::StreamExt;

use crate::engine_actor::EngineHandle;
use tokio::sync::Mutex;
use tokio::sync::watch;
use tower_sessions::cookie::time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, ExpiredDeletion, SessionStore};

use rustls_acme::caches::DirCache;
use rustls_acme::{AcmeConfig, UseChallenge};

/// Everything `tls.rs` needs to stand up the ACME listeners, lifted out of the
/// `dux_core::config::ServerPlan::Acme` variant by the binary so this module
/// never depends on dux-core's plan enum directly.
#[derive(Clone, Debug)]
pub struct AcmePlan {
    pub http_addr: SocketAddr,
    pub https_addr: SocketAddr,
    /// RAW domains as resolved by dux-core — normalized here via
    /// [`normalize_domains`] before issuance and allowlisting.
    pub domains: Vec<String>,
    pub email: String,
    pub production: bool,
    pub cache_dir: std::path::PathBuf,
}

// ── Domain normalization (the one shared contract) ─────────────────────────

/// Normalize a single domain for use in BOTH the Host allowlist and ACME
/// issuance. The contract (deliberately small — anything beyond this is a config
/// error the caller must surface, not silently fix):
///
/// 1. trim surrounding whitespace,
/// 2. strip a leading `http://` or `https://` scheme (a common copy-paste slip),
/// 3. lowercase (DNS is case-insensitive; the Host comparison must be too),
/// 4. drop a single trailing dot (the fully-qualified `example.com.` form),
/// 5. an empty result (a blank entry) maps to `None` so the caller can drop it.
///
/// It does NOT strip ports or paths. After step 2 a remaining `/` or `:` means
/// the entry was malformed (e.g. `dux.example.com/app` or `dux.example.com:8443`):
/// returns `Err` so [`normalize_domains`] can refuse startup with a named error
/// rather than guessing what the operator meant.
///
/// Module-private: only [`normalize_domains`] (the list form) is the exported
/// contract; the binary's URL display and every caller use the plural form.
fn normalize_domain(raw: &str) -> Result<Option<String>, String> {
    let trimmed = raw.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let lowered = without_scheme.to_ascii_lowercase();
    // A trailing dot is the valid FQDN form; drop exactly one so it matches the
    // Host header a browser sends (which never carries the trailing dot).
    let no_trailing_dot = lowered.strip_suffix('.').unwrap_or(&lowered);
    if no_trailing_dot.is_empty() {
        return Ok(None);
    }
    if no_trailing_dot.contains('/') || no_trailing_dot.contains(':') {
        return Err(format!(
            "invalid ACME domain \"{raw}\": expected a bare hostname like dux.example.com \
             (no scheme, no port, no path). A leading http:// or https:// is stripped \
             automatically, but a remaining '/' or ':' is ambiguous — remove the port or \
             path."
        ));
    }
    Ok(Some(no_trailing_dot.to_string()))
}

/// Normalize a list of domains: apply [`normalize_domain`] to each, drop blanks,
/// and DEDUP while preserving first-seen order (so the certificate's primary name
/// stays the operator's first domain). Returns `Err` on the first malformed entry
/// and `Err` when nothing usable remains (an all-blank list is a misconfiguration
/// the caller must hear about, not silently serve with no SAN).
pub fn normalize_domains(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    for entry in raw {
        if let Some(domain) = normalize_domain(entry)?
            && !out.contains(&domain)
        {
            out.push(domain);
        }
    }
    if out.is_empty() {
        return Err(
            "no usable ACME domains after normalization: every configured domain was blank. \
             Configure at least one hostname in [server.acme] domains (for example \
             dux.example.com)."
                .to_string(),
        );
    }
    Ok(out)
}

// ── ACME state + the polling task ──────────────────────────────────────────

/// The concrete `AcmeState` type for the `DirCache` backend we use. `DirCache`'s
/// load/store errors are `std::io::Error` for both the cert (`EC`) and account
/// (`EA`) caches, so the state's error parameters are both `std::io::Error`.
pub type DuxAcmeState = rustls_acme::AcmeState<std::io::Error, std::io::Error>;

/// Build the live `AcmeState` from the plan. Creates the cache directory at
/// `0700` BEFORE handing it to `DirCache` — it stores the ACME account key and
/// the issued certificate's private key, so it must never be group/other
/// readable. Returns the normalized domains alongside the state so the caller can
/// reuse them for the Host allowlist and URL display without re-normalizing.
pub fn build_acme_state(plan: &AcmePlan) -> anyhow::Result<(DuxAcmeState, Vec<String>)> {
    let domains = normalize_domains(&plan.domains).map_err(|e| anyhow::anyhow!(e))?;

    // The cache dir holds PRIVATE keys. Create it (and parents) then tighten the
    // leaf to 0700 so a umask that left it world-readable is corrected. Existing
    // dirs are re-tightened too (idempotent, cheap, and fixes a loosened dir).
    std::fs::create_dir_all(&plan.cache_dir).map_err(|e| {
        anyhow::anyhow!(
            "failed to create ACME cache directory {}: {e}",
            plan.cache_dir.display()
        )
    })?;
    set_dir_private(&plan.cache_dir)?;

    let contact: Vec<String> = if plan.email.trim().is_empty() {
        Vec::new()
    } else {
        vec![format!("mailto:{}", plan.email.trim())]
    };

    let state = AcmeConfig::new(domains.clone())
        .contact(contact)
        .cache_option(Some(DirCache::new(plan.cache_dir.clone())))
        .directory_lets_encrypt(plan.production)
        .challenge_type(UseChallenge::Http01)
        .state();

    Ok((state, domains))
}

/// Set a directory to `0700` (owner-only). Unix-only by project policy.
fn set_dir_private(dir: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(dir, perms).map_err(|e| {
        anyhow::anyhow!(
            "failed to set 0700 permissions on the ACME cache directory {} \
             (it holds private keys): {e}",
            dir.display()
        )
    })
}

/// The status-relevant classification of one ACME certificate-lifecycle event,
/// decoupled from rustls-acme's generic `Event<EC, EA>` type so the mapping to a
/// user-facing [`WireStatus`] is a pure, exhaustively unit-testable function. The
/// polling loop projects each concrete event into one of these BEFORE it touches
/// the status broadcast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcmeStatusEvent {
    /// A certificate was deployed (newly issued/renewed, or a valid cached one
    /// loaded at boot). The user-visible milestone.
    CertDeployed,
    /// Internal cache bookkeeping (a cert or account key was persisted). Logged,
    /// but not surfaced on the status line — it is not a lifecycle milestone.
    CacheStore,
    /// A lifecycle error (cache load/store, order, or parse). Renewal will retry.
    Error(String),
    /// The lifecycle stream ended — certificates will no longer renew.
    StreamEnded,
}

/// Map an [`AcmeStatusEvent`] to the status-line update (if any) it should
/// broadcast to web clients. Pure so it is exhaustively unit-tested.
///
/// - `CertDeployed` → info: certificates are live for `domains`.
/// - `CacheStore` → `None`: logged only, not a user-facing milestone.
/// - `Error` / `StreamEnded` → error: surfaced so an operator sees a failing
///   renewal on the status line, with `dux.log` named for the detail.
pub fn acme_status_for_event(event: &AcmeStatusEvent, domains: &[String]) -> Option<WireStatus> {
    match event {
        AcmeStatusEvent::CertDeployed => Some(
            WireStatus::new(
                "info",
                format!(
                    "TLS certificate acquired/renewed for {}.",
                    domains.join(", ")
                ),
            )
            .with_key("acme"),
        ),
        AcmeStatusEvent::CacheStore => None,
        AcmeStatusEvent::Error(e) => Some(
            WireStatus::new(
                "error",
                format!("ACME certificate error: {e} — renewal will retry; see dux.log."),
            )
            .with_key("acme"),
        ),
        AcmeStatusEvent::StreamEnded => Some(
            WireStatus::new(
                "error",
                "ACME certificate lifecycle stopped — certificates will no longer renew. \
                 Restart dux to recover automatic renewal; see dux.log."
                    .to_string(),
            )
            .with_key("acme"),
        ),
    }
}

/// Spawn the dedicated task that drives the `AcmeState` stream. rustls-acme does
/// NO background work itself — certificate acquisition and renewal only advance
/// while this stream is polled. Per the explicit-failure tenet every event is
/// logged: `Ok` at info (acquired/renewed/cached), `Err` at error (loud, so a
/// failing renewal is visible in `dux.log`), and a stream END is an error too
/// because it means certificates will silently stop renewing.
///
/// `emit` is the engine handle. When `Some`, each event is projected through
/// [`acme_status_for_event`] and published through the engine's shared status
/// controller (via [`EngineHandle::emit_status`]) so the web UI shows certificate
/// acquisition/renewal/failure live AND it auto-clears on the same policy as
/// every other status — not just `dux.log`. `None` for callers without an engine
/// actor (tests, standalone construction).
///
/// `console` is the `dux server` terminal console (a [`crate::console::Console`]):
/// each surfaced event is ALSO echoed there so an operator watching the terminal
/// sees the certificate lifecycle in the vite-style output. A no-op console (the
/// flip/tests) emits nothing. The console line and the `WireStatus` carry the
/// SAME text (from the pure mapper) so the surfaces can never drift.
///
/// `domains` is the normalized certificate name list (from [`build_acme_state`]),
/// used only for the acquired/renewed status text.
///
/// Returns the `JoinHandle` so the serve path can abort it on shutdown.
pub fn spawn_acme_event_task(
    mut state: DuxAcmeState,
    emit: Option<EngineHandle>,
    console: crate::console::Console,
    domains: Vec<String>,
) -> tokio::task::JoinHandle<()> {
    // Echo a surfaced status to BOTH the engine's status controller and the
    // console, so the web UI and the terminal stay in lock-step with dux.log.
    fn surface(emit: &Option<EngineHandle>, console: &crate::console::Console, status: WireStatus) {
        console.acme(status.tone == "error", &status.message);
        if let Some(handle) = emit {
            handle.emit_status(status);
        }
    }

    tokio::spawn(async move {
        loop {
            // Project the concrete event into the status classification first, then
            // log AND surface — the surfaced text comes from the pure mapper so the
            // log, the status broadcast, and the console can never drift.
            let event = match state.next().await {
                Some(Ok(ok)) => {
                    dux_core::logger::info(&format!(
                        "[acme] certificate lifecycle event: {ok:?} — TLS certificates are \
                         being acquired/renewed via Let's Encrypt."
                    ));
                    classify_event_ok(&ok)
                }
                Some(Err(err)) => {
                    dux_core::logger::error(&format!(
                        "[acme] certificate lifecycle ERROR: {err} — TLS may be using a stale or \
                         missing certificate. Check that :80 is reachable from the internet for \
                         HTTP-01 validation and that the domain's DNS points here."
                    ));
                    AcmeStatusEvent::Error(err.to_string())
                }
                None => {
                    dux_core::logger::error(
                        "[acme] the certificate lifecycle stream ended — certificates will NOT \
                         renew. The HTTPS server keeps serving the last certificate until it \
                         expires; restart dux to recover automatic renewal.",
                    );
                    if let Some(status) =
                        acme_status_for_event(&AcmeStatusEvent::StreamEnded, &domains)
                    {
                        surface(&emit, &console, status);
                    }
                    break;
                }
            };
            if let Some(status) = acme_status_for_event(&event, &domains) {
                surface(&emit, &console, status);
            }
        }
    })
}

/// Classify a rustls-acme `EventOk` into the status-relevant [`AcmeStatusEvent`].
/// A deploy (new or cached) is the user-visible milestone; a cache store is
/// internal bookkeeping.
fn classify_event_ok(ok: &rustls_acme::EventOk) -> AcmeStatusEvent {
    use rustls_acme::EventOk;
    match ok {
        EventOk::DeployedCachedCert | EventOk::DeployedNewCert => AcmeStatusEvent::CertDeployed,
        EventOk::CertCacheStore | EventOk::AccountCacheStore => AcmeStatusEvent::CacheStore,
    }
}

// ── :80 challenge + redirect router ────────────────────────────────────────

/// Strip a `:port` from a (trimmed, non-empty) `Host` value, bracket-aware for
/// IPv6, returning the bare host (IPv6 kept bracketed). The ONE host-port parser
/// shared by [`redirect_target`] (the `:80` redirect Location) and
/// [`normalize_host_for_match`] (the allowlist comparison), so the redirect target
/// and the accepted Host can never disagree on what the authority is.
///
/// - bracketed IPv6 (`[::1]` / `[::1]:80`) → the bracketed literal, port dropped;
///   a missing closing bracket is malformed (`None`).
/// - bare host / IPv4 with a trailing all-digit `:port` → the host, port dropped.
/// - an unbracketed multi-colon value (an unbracketed IPv6) is malformed (`None`).
/// - anything else → unchanged.
fn strip_host_port(host: &str) -> Option<String> {
    if let Some(rest) = host.strip_prefix('[') {
        // Bracketed IPv6: keep through the closing bracket, drop a trailing :port.
        let close = rest.find(']')?;
        Some(format!("[{}]", &rest[..close]))
    } else {
        // Bare host or IPv4. A trailing :port is the part after the LAST colon
        // when that tail is all digits. A host with multiple colons but no
        // brackets is malformed (an unbracketed IPv6) — reject it.
        match host.rsplit_once(':') {
            Some((left, right))
                if right.chars().all(|c| c.is_ascii_digit()) && !right.is_empty() =>
            {
                if left.contains(':') {
                    return None; // unbracketed IPv6 with port — malformed
                }
                Some(left.to_string())
            }
            Some((left, _)) if left.contains(':') => None, // unbracketed IPv6
            _ => Some(host.to_string()),
        }
    }
}

/// Compute the HTTPS redirect target for a plaintext `:80` request. Pure so it is
/// exhaustively unit-tested.
///
/// - `host_header` is the incoming `Host` (may carry a `:port` we strip; may be a
///   bracketed IPv6 literal we preserve bracketed).
/// - `https_port` is the port the TLS listener serves on; omitted from the URL
///   when it is the default 443.
/// - `uri` carries the path + query to preserve.
///
/// Returns `None` for a missing or syntactically unusable host so the caller can
/// answer `400` instead of emitting a `Location` pointing at an attacker-chosen
/// or empty authority.
///
/// Module-private: only the `:80` redirect handler in this module calls it.
fn redirect_target(host_header: Option<&str>, https_port: u16, uri: &Uri) -> Option<String> {
    let host = host_header?.trim();
    if host.is_empty() {
        return None;
    }
    let host_no_port = strip_host_port(host)?;
    if host_no_port.is_empty() || host_no_port == "[]" {
        return None;
    }
    let authority = if https_port == 443 {
        host_no_port
    } else {
        format!("{host_no_port}:{https_port}")
    };
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    Some(format!("https://{authority}{path_and_query}"))
}

/// State carried by the `:80` redirect fallback handler.
#[derive(Clone)]
struct RedirectState {
    https_port: u16,
}

/// Fallback handler for every `:80` request that is NOT an ACME challenge: a
/// permanent (308) redirect to the HTTPS equivalent, preserving path + query and
/// stripping any port from the Host. A malformed/missing Host gets a 400 rather
/// than a redirect to a bogus authority.
async fn redirect_to_https(
    State(state): State<RedirectState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok());
    match redirect_target(host, state.https_port, &uri) {
        Some(target) => Redirect::permanent(&target).into_response(),
        None => (
            StatusCode::BAD_REQUEST,
            "missing or invalid Host header; cannot redirect to HTTPS",
        )
            .into_response(),
    }
}

/// Build the `:80` router: the real HTTP-01 challenge tower service mounted at
/// `/.well-known/acme-challenge/{token}` (EXEMPT from the Host allowlist — it is
/// token-keyed, returns 404 for unknown tokens, and the CA dials it by IP, often
/// with a Host that is not one of our domains) and a fallback that redirects
/// everything else to HTTPS.
///
/// Construction (the part that makes the exemption REAL, not just commented):
/// `Router::layer` in axum 0.8 wraps EVERY route in the router INCLUDING any
/// `route_service` and the fallback — so layering the allowlist onto the whole
/// `:80` router would 421 a foreign-Host challenge probe and silently break
/// issuance/renewal. Instead the allowlist is scoped to a dedicated redirect
/// sub-router (its `.layer()` wraps only that sub-router's fallback), which is
/// then mounted via `fallback_service`. The challenge `route_service` lives on
/// the OUTER router, which carries no allowlist layer, so a challenge request with
/// any Host reaches the real tower service untouched.
///
/// `console` + `access_log` wire the SAME access log the main app uses
/// ([`crate::server::access_log_layer`]) onto this router, applied OUTERMOST so it
/// records the final status of every `:80` request — a 200/404 challenge fetch, a
/// 308 redirect, a 421 from the allowlist, or a 400 bad-Host. Gated on
/// `access_log && console.is_active()`, so the flip/disabled paths log nothing.
pub fn build_http_challenge_router(
    state: &DuxAcmeState,
    https_port: u16,
    domains: Vec<String>,
    console: crate::console::Console,
    access_log: bool,
) -> Router {
    let challenge = state.http01_challenge_tower_service();
    let allowlist = Arc::new(DomainAllowlist::new(domains));
    // The redirect sub-router: the allowlist guards its fallback so a rebinding
    // attacker cannot coax a redirect that legitimizes their hostname. The
    // `.layer()` here wraps only THIS sub-router (its lone fallback), never the
    // challenge route on the outer router below.
    let redirect = Router::new()
        .fallback(redirect_to_https)
        .layer(axum::middleware::from_fn_with_state(
            allowlist,
            host_allowlist_middleware,
        ))
        .with_state(RedirectState { https_port });
    // The outer router carries no ALLOWLIST layer (the challenge route_service is
    // genuinely exempt, and everything else falls through to the allowlisted
    // redirect sub-router); the access log is then layered OUTERMOST over the whole
    // thing so every `:80` request — challenge or redirect, allowed or 421'd —
    // logs its final status.
    let router = Router::new()
        .route_service("/.well-known/acme-challenge/{token}", challenge)
        .fallback_service(redirect);
    crate::server::access_log_layer(router, console, access_log)
}

// ── Host allowlist (deferral 2) ────────────────────────────────────────────

/// The set of Host values a request may carry when dux terminates TLS. Built from
/// the SAME normalized domains used for issuance, so the cert names and the
/// accepted Hosts can never drift.
///
/// Module-private: it is an implementation detail of [`host_allowlist_layer`] and
/// [`build_http_challenge_router`]; callers wire the allowlist through those.
#[derive(Debug, Clone)]
struct DomainAllowlist {
    domains: Vec<String>,
}

impl DomainAllowlist {
    /// `domains` MUST already be normalized (via [`normalize_domains`]).
    fn new(domains: Vec<String>) -> Self {
        Self { domains }
    }

    /// Whether a raw `Host` header value is allowed: strip any `:port`, drop a
    /// trailing dot, lowercase, then membership-test against the normalized set.
    fn allows(&self, host_header: &str) -> bool {
        let Some(host) = normalize_host_for_match(host_header) else {
            return false;
        };
        self.domains.contains(&host)
    }
}

/// Normalize an incoming `Host` header for allowlist comparison: strip a `:port`
/// (bracket-aware for IPv6, via the shared [`strip_host_port`]), drop a single
/// trailing dot, lowercase. Sharing [`strip_host_port`] with [`redirect_target`]
/// guarantees the allowlist and the redirect agree on the authority.
fn normalize_host_for_match(host_header: &str) -> Option<String> {
    let host = host_header.trim();
    if host.is_empty() {
        return None;
    }
    let host_no_port = strip_host_port(host)?;
    let lowered = host_no_port.to_ascii_lowercase();
    let no_dot = lowered.strip_suffix('.').unwrap_or(&lowered);
    if no_dot.is_empty() {
        None
    } else {
        Some(no_dot.to_string())
    }
}

/// Middleware: pin requests to the configured domains. A request whose `Host` is
/// present but not in the allowlist gets `421 Misdirected Request` (the
/// semantically correct code for "this server is not authoritative for the
/// requested host") — the DNS-rebinding defense: an attacker who points a
/// controlled hostname at this IP gets a 421 instead of a response that would let
/// their page talk to dux. A request with NO usable `Host` header (absent, or not
/// valid UTF-8) is malformed per HTTP/1.1 rather than misrouted, so it gets
/// `400 Bad Request` instead of a 421 (which a client may read as "retry on a new
/// connection").
async fn host_allowlist_middleware(
    State(allowlist): State<Arc<DomainAllowlist>>,
    request: Request,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok());
    match host {
        Some(h) if allowlist.allows(h) => next.run(request).await,
        // A Host that normalizes to nothing (empty or whitespace-only) carries no
        // hostname — malformed like an absent header, so 400 rather than 421.
        Some(h) if normalize_host_for_match(h).is_none() => {
            (StatusCode::BAD_REQUEST, "missing or invalid Host header").into_response()
        }
        Some(_) => (
            StatusCode::MISDIRECTED_REQUEST,
            "this dux server does not serve the requested host",
        )
            .into_response(),
        None => (StatusCode::BAD_REQUEST, "missing or invalid Host header").into_response(),
    }
}

/// Wrap a router with the Host allowlist middleware. Used for the HTTPS app so
/// every gated and open route alike is pinned to the configured domains.
pub fn host_allowlist_layer(router: Router, domains: Vec<String>) -> Router {
    let allowlist = Arc::new(DomainAllowlist::new(domains));
    router.layer(axum::middleware::from_fn_with_state(
        allowlist,
        host_allowlist_middleware,
    ))
}

// ── HSTS (HTTPS-only) ──────────────────────────────────────────────────────

/// The `Strict-Transport-Security` value dux sends on the TLS path: a two-year
/// `max-age` with `includeSubDomains`, but NO `preload`.
///
/// - `max-age=63072000` — 2 years, comfortably above the 1-year minimum that the
///   HSTS preload list and Lighthouse's HSTS audit require, long enough that a
///   returning browser keeps upgrading to HTTPS on its own.
/// - `includeSubDomains` — every name under the served domain is HTTPS-only too.
/// - NO `preload` — preload is an irreversible, browser-vendor-list commitment
///   that an operator must opt into deliberately, never something dux asserts on
///   their behalf.
pub const HSTS_HEADER_VALUE: &str = "max-age=63072000; includeSubDomains";

/// Middleware that stamps `Strict-Transport-Security` on every response. Used
/// ONLY on the HTTPS app dux serves over its own TLS (the ACME path), never on the
/// plain-HTTP/proxy/flip paths: HSTS instructs the browser to refuse plain HTTP to
/// this host, which is only correct when dux actually terminates TLS. On a
/// proxy-fronted or loopback deployment dux does not own TLS, so it must not make
/// that promise.
async fn hsts_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::header::STRICT_TRANSPORT_SECURITY,
        axum::http::HeaderValue::from_static(HSTS_HEADER_VALUE),
    );
    response
}

/// Wrap a router with the HSTS header middleware. Applied to the HTTPS app ONLY
/// (the `RouterParams::tls()` path), next to [`host_allowlist_layer`] in
/// `run_acme`. NEVER applied to the plain-HTTP/proxy/flip apps, where dux does not
/// own TLS and must not tell the browser this host is HTTPS-only.
pub fn hsts_layer(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(hsts_middleware))
}

// ── Session sweep (deferral 3) ─────────────────────────────────────────────

/// An in-memory session store that the periodic sweep can prune.
///
/// `tower_sessions::MemoryStore` keeps an `Arc<Mutex<HashMap<Id, Record>>>` whose
/// `load` already treats expired records as absent — but it NEVER removes them,
/// so a long-lived server with login churn accumulates dead entries forever. That
/// store does not implement `ExpiredDeletion` and hides its map, so there is no
/// way to bolt a sweep onto it. This is the smallest correct fix: a byte-for-byte
/// reimplementation of `MemoryStore`'s `SessionStore` logic (the documented
/// reference impl) over a map WE own, plus an `ExpiredDeletion::delete_expired`
/// that evicts every record whose `expiry_date` has passed. No session crypto is
/// reimplemented — signing, cookie handling, and id rotation stay in
/// `tower_sessions`'s `SessionManagerLayer`; this only owns the storage map.
#[derive(Clone, Debug, Default)]
pub struct SweepableMemoryStore(Arc<Mutex<HashMap<Id, Record>>>);

impl SweepableMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current record count — test-only visibility into the sweep effect.
    /// (Named `record_count` rather than `len` to avoid the misleading-impl lint;
    /// emptiness is not a meaningful query for a session store.)
    #[cfg(test)]
    pub async fn record_count(&self) -> usize {
        self.0.lock().await.len()
    }
}

#[async_trait]
impl SessionStore for SweepableMemoryStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let mut guard = self.0.lock().await;
        while guard.contains_key(&record.id) {
            // Id collision mitigation, exactly as MemoryStore does it.
            record.id = Id::default();
        }
        guard.insert(record.id, record.clone());
        Ok(())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        self.0.lock().await.insert(record.id, record.clone());
        Ok(())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        Ok(self
            .0
            .lock()
            .await
            .get(session_id)
            .filter(|Record { expiry_date, .. }| *expiry_date > OffsetDateTime::now_utc())
            .cloned())
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        self.0.lock().await.remove(session_id);
        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for SweepableMemoryStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let now = OffsetDateTime::now_utc();
        self.0.lock().await.retain(|_, rec| rec.expiry_date > now);
        Ok(())
    }
}

/// Default sweep cadence: hourly. Expired sessions are already inert (`load`
/// skips them), so this is purely about bounding memory; an hour keeps the map
/// small without burning cycles.
pub const SESSION_SWEEP_PERIOD: Duration = Duration::from_secs(60 * 60);

/// Spawn the periodic expired-session sweep. Runs `delete_expired` every
/// `period`, and STOPS as soon as `shutdown` flips to `true` so it winds down
/// with the server (no orphaned task surviving a flip or a quit). Returns the
/// task handle.
pub fn spawn_session_sweep(
    store: SweepableMemoryStore,
    period: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        // Burn the immediate first tick so the first real sweep is one period in
        // (a fresh store has nothing to prune).
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = store.delete_expired().await {
                        dux_core::logger::warn(&format!(
                            "[sessions] expired-session sweep failed: {e}"
                        ));
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

// ── HTTPS serving (injectable acceptor) ────────────────────────────────────

/// Serve `app` over axum-server with the production rustls-acme acceptor and a
/// graceful-shutdown handle, preserving per-connection peer info (connect-info)
/// so the per-IP login rate limiter keeps working. The caller drives
/// `handle.graceful_shutdown(..)` from the shared shutdown lane, so an `Ok(())`
/// return means a clean wind-down and an `Err` means the accept loop genuinely
/// died.
///
/// Production binds by address; this is a thin wrapper that binds then delegates
/// to [`serve_https_with_acceptor`], the SHARED core the e2e drives with a
/// pre-bound ephemeral listener and a self-signed `RustlsAcceptor`. The ONLY
/// difference between prod and test is the acceptor object and who binds — the
/// `into_make_service_with_connect_info` serve path is shared, so no e2e can pass
/// while production silently drops connect-info.
pub async fn serve_https_acme(
    addr: SocketAddr,
    app: Router,
    acceptor: rustls_acme::axum::AxumAcceptor,
    handle: axum_server::Handle<SocketAddr>,
) -> std::io::Result<()> {
    let listener = std::net::TcpListener::bind(addr)?;
    serve_https_with_acceptor(listener, app, acceptor, handle).await
}

/// Shared HTTPS serve core: serve `app` over axum-server on a PRE-BOUND std
/// listener with the given acceptor, connect-info, and graceful-shutdown handle.
/// Production reaches this via [`serve_https_acme`] (bind-by-addr → here with
/// rustls-acme's `AxumAcceptor`); the e2e reaches it directly with a self-signed
/// `RustlsAcceptor` on a `127.0.0.1:0` listener. The acceptor is generic over the
/// axum-server `Accept` contract so both injections share this exact serve path.
///
/// Wraps an axum-server acceptor to disable Nagle (TCP_NODELAY) on each accepted
/// socket before the TLS handshake. Terminal traffic is many tiny packets
/// (keystrokes, per-char echo/redraws); without this, Nagle batches them into
/// laggy clumps that make remote typing stutter and flicker. Forwards every
/// associated type to the inner acceptor — it only side-effects the raw stream.
#[derive(Clone)]
struct NoDelayAcceptor<A>(A);

impl<A, S> axum_server::accept::Accept<tokio::net::TcpStream, S> for NoDelayAcceptor<A>
where
    A: axum_server::accept::Accept<tokio::net::TcpStream, S>,
{
    type Stream = A::Stream;
    type Service = A::Service;
    type Future = A::Future;

    fn accept(&self, stream: tokio::net::TcpStream, service: S) -> Self::Future {
        let _ = stream.set_nodelay(true);
        self.0.accept(stream, service)
    }
}

/// The std listener must be non-blocking before tokio registers it; this sets it
/// so callers (prod and test alike) need not remember to.
pub async fn serve_https_with_acceptor<A, S>(
    listener: std::net::TcpListener,
    app: Router,
    acceptor: A,
    handle: axum_server::Handle<SocketAddr>,
) -> std::io::Result<()>
where
    // The make-service produces a per-connection service `S` (the router with
    // connect-info injected); the acceptor must accept a `TcpStream` and pass that
    // `S` through. Both the production `AxumAcceptor` and the e2e's
    // `RustlsAcceptor` are service-preserving (`Service = S`), so this is exactly
    // axum-server's own `serve` contract specialized to our connect-info app.
    A: axum_server::accept::Accept<tokio::net::TcpStream, S, Service = S>
        + Clone
        + Send
        + Sync
        + 'static,
    A::Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
    A::Future: Send,
    axum::extract::connect_info::IntoMakeServiceWithConnectInfo<Router, SocketAddr>:
        axum_server::service::MakeService<
                SocketAddr,
                axum::http::Request<hyper::body::Incoming>,
                Service = S,
            >,
    S: axum_server::service::SendService<axum::http::Request<hyper::body::Incoming>>
        + Send
        + 'static,
{
    listener.set_nonblocking(true)?;
    axum_server::from_tcp(listener)?
        .handle(handle)
        .acceptor(NoDelayAcceptor(acceptor))
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
}

/// Serve the plain `:80` challenge + redirect router with a graceful-shutdown
/// handle and connect-info (kept for parity even though the redirect path does
/// not read the peer). Returns the accept loop's `io::Result`.
///
/// Production binds by address; this wrapper binds then delegates to
/// [`serve_http_challenge_from_tcp`], the SHARED core the e2e drives with a
/// pre-bound ephemeral listener — so the e2e exercises the production serve path
/// (connect-info included), not an inline copy of it.
pub async fn serve_http_challenge(
    addr: SocketAddr,
    app: Router,
    handle: axum_server::Handle<SocketAddr>,
) -> std::io::Result<()> {
    let listener = std::net::TcpListener::bind(addr)?;
    serve_http_challenge_from_tcp(listener, app, handle).await
}

/// Shared `:80` serve core: serve the challenge + redirect router over
/// axum-server on a PRE-BOUND std listener with connect-info and a
/// graceful-shutdown handle. Production reaches it via [`serve_http_challenge`]
/// (bind-by-addr → here); the e2e reaches it directly with a `127.0.0.1:0`
/// listener. Sets the listener non-blocking so callers need not.
///
/// Unlike [`serve_https_with_acceptor`], this path does NOT wrap the acceptor in
/// `NoDelayAcceptor`: `:80` only carries ACME challenge probes and one-shot 308
/// redirects, never the interactive terminal stream that TCP_NODELAY exists to
/// de-jitter, so Nagle batching is harmless here.
pub async fn serve_http_challenge_from_tcp(
    listener: std::net::TcpListener,
    app: Router,
    handle: axum_server::Handle<SocketAddr>,
) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    axum_server::from_tcp(listener)?
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_domain ──────────────────────────────────────────────────

    #[test]
    fn normalize_domain_lowercases_and_trims() {
        assert_eq!(
            normalize_domain("  DUX.Example.COM  ").unwrap(),
            Some("dux.example.com".to_string())
        );
    }

    #[test]
    fn normalize_domain_strips_scheme_prefix() {
        assert_eq!(
            normalize_domain("https://dux.example.com").unwrap(),
            Some("dux.example.com".to_string())
        );
        assert_eq!(
            normalize_domain("http://dux.example.com").unwrap(),
            Some("dux.example.com".to_string())
        );
    }

    #[test]
    fn normalize_domain_drops_trailing_dot() {
        assert_eq!(
            normalize_domain("dux.example.com.").unwrap(),
            Some("dux.example.com".to_string())
        );
    }

    #[test]
    fn normalize_domain_blank_is_none() {
        assert_eq!(normalize_domain("").unwrap(), None);
        assert_eq!(normalize_domain("   ").unwrap(), None);
    }

    #[test]
    fn normalize_domain_rejects_remaining_path_or_port() {
        // A path after stripping the scheme is ambiguous → named error.
        assert!(normalize_domain("https://dux.example.com/app").is_err());
        // A port is likewise rejected (not silently stripped).
        assert!(normalize_domain("dux.example.com:8443").is_err());
        // A bare slash is rejected too.
        assert!(normalize_domain("dux.example.com/").is_err());
    }

    // ── normalize_domains (list) ──────────────────────────────────────────

    #[test]
    fn normalize_domains_dedups_preserving_order() {
        let raw = vec![
            "Dux.Example.com".to_string(),
            "api.example.com".to_string(),
            "dux.example.com".to_string(), // dup of first after fold
            "  ".to_string(),              // blank, dropped
        ];
        assert_eq!(
            normalize_domains(&raw).unwrap(),
            vec!["dux.example.com".to_string(), "api.example.com".to_string()]
        );
    }

    #[test]
    fn normalize_domains_all_blank_is_error() {
        let raw = vec!["".to_string(), "   ".to_string()];
        assert!(normalize_domains(&raw).is_err());
    }

    #[test]
    fn normalize_domains_propagates_malformed_entry() {
        let raw = vec![
            "ok.example.com".to_string(),
            "bad.example.com:9".to_string(),
        ];
        assert!(normalize_domains(&raw).is_err());
    }

    // ── strip_host_port (shared by redirect + allowlist) ──────────────────

    #[test]
    fn strip_host_port_handles_bare_ipv6_and_brackets() {
        // Bare host / IPv4: trailing all-digit port dropped, else unchanged.
        assert_eq!(
            strip_host_port("dux.example.com"),
            Some("dux.example.com".to_string())
        );
        assert_eq!(
            strip_host_port("dux.example.com:443"),
            Some("dux.example.com".to_string())
        );
        assert_eq!(
            strip_host_port("10.0.0.1:8443"),
            Some("10.0.0.1".to_string())
        );
        // Bracketed IPv6 keeps the brackets, drops the port.
        assert_eq!(strip_host_port("[::1]"), Some("[::1]".to_string()));
        assert_eq!(
            strip_host_port("[2001:db8::1]:80"),
            Some("[2001:db8::1]".to_string())
        );
        // Unbracketed IPv6 (with or without a port) is malformed.
        assert_eq!(strip_host_port("2001:db8::1"), None);
        assert_eq!(strip_host_port("2001:db8::1:443"), None);
        // A missing closing bracket is malformed.
        assert_eq!(strip_host_port("[::1"), None);
    }

    // ── redirect_target ───────────────────────────────────────────────────

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn redirect_target_default_port_omits_suffix() {
        let got = redirect_target(Some("dux.example.com"), 443, &uri("/path?x=1"));
        assert_eq!(got.as_deref(), Some("https://dux.example.com/path?x=1"));
    }

    #[test]
    fn redirect_target_non_default_port_includes_suffix() {
        let got = redirect_target(Some("dux.example.com"), 8443, &uri("/"));
        assert_eq!(got.as_deref(), Some("https://dux.example.com:8443/"));
    }

    #[test]
    fn redirect_target_strips_incoming_port() {
        // The :80 in the Host must be dropped before re-adding the https port.
        let got = redirect_target(Some("dux.example.com:80"), 443, &uri("/a"));
        assert_eq!(got.as_deref(), Some("https://dux.example.com/a"));
        let got = redirect_target(Some("dux.example.com:80"), 8443, &uri("/a"));
        assert_eq!(got.as_deref(), Some("https://dux.example.com:8443/a"));
    }

    #[test]
    fn redirect_target_preserves_query() {
        let got = redirect_target(Some("dux.example.com"), 443, &uri("/x?a=1&b=2"));
        assert_eq!(got.as_deref(), Some("https://dux.example.com/x?a=1&b=2"));
    }

    #[test]
    fn redirect_target_ipv6_host_stays_bracketed() {
        let got = redirect_target(Some("[2001:db8::1]:80"), 443, &uri("/p"));
        assert_eq!(got.as_deref(), Some("https://[2001:db8::1]/p"));
        let got = redirect_target(Some("[2001:db8::1]"), 8443, &uri("/p"));
        assert_eq!(got.as_deref(), Some("https://[2001:db8::1]:8443/p"));
    }

    #[test]
    fn redirect_target_missing_or_garbage_host_rejected() {
        assert_eq!(redirect_target(None, 443, &uri("/")), None);
        assert_eq!(redirect_target(Some(""), 443, &uri("/")), None);
        assert_eq!(redirect_target(Some("   "), 443, &uri("/")), None);
        // Unbracketed IPv6 is malformed → rejected (no Location to a bad host).
        assert_eq!(redirect_target(Some("2001:db8::1"), 443, &uri("/")), None);
        assert_eq!(
            redirect_target(Some("2001:db8::1:443"), 443, &uri("/")),
            None
        );
    }

    #[test]
    fn redirect_target_no_path_defaults_to_slash() {
        let got = redirect_target(Some("dux.example.com"), 443, &uri("/"));
        assert_eq!(got.as_deref(), Some("https://dux.example.com/"));
    }

    // ── DomainAllowlist ───────────────────────────────────────────────────

    #[test]
    fn allowlist_matches_case_insensitively_and_strips_port() {
        let al = DomainAllowlist::new(vec!["dux.example.com".to_string()]);
        assert!(al.allows("dux.example.com"));
        assert!(al.allows("DUX.Example.com"));
        assert!(al.allows("dux.example.com:443"));
        assert!(al.allows("dux.example.com."));
    }

    #[test]
    fn allowlist_rejects_unknown_and_empty_host() {
        let al = DomainAllowlist::new(vec!["dux.example.com".to_string()]);
        assert!(!al.allows("evil.example.com"));
        assert!(!al.allows(""));
        assert!(!al.allows("   "));
        // An IP that resolves to this server via rebinding is not in the set.
        assert!(!al.allows("203.0.113.5"));
    }

    // ── SweepableMemoryStore + sweep ──────────────────────────────────────

    fn record_expiring(offset_secs: i64) -> Record {
        Record {
            id: Id::default(),
            data: Default::default(),
            expiry_date: OffsetDateTime::now_utc()
                + tower_sessions::cookie::time::Duration::seconds(offset_secs),
        }
    }

    #[tokio::test]
    async fn store_load_skips_expired_but_delete_expired_evicts() {
        let store = SweepableMemoryStore::new();
        let mut live = record_expiring(3600);
        let mut dead = record_expiring(-1);
        store.create(&mut live).await.unwrap();
        store.create(&mut dead).await.unwrap();
        assert_eq!(
            store.record_count().await,
            2,
            "both records are present before sweep"
        );

        // load() already treats the expired one as absent (MemoryStore parity)...
        assert!(store.load(&live.id).await.unwrap().is_some());
        assert!(store.load(&dead.id).await.unwrap().is_none());
        // ...but it lingers in the map until the sweep removes it.
        assert_eq!(
            store.record_count().await,
            2,
            "expired record still occupies memory"
        );

        store.delete_expired().await.unwrap();
        assert_eq!(
            store.record_count().await,
            1,
            "sweep evicts the expired record"
        );
        assert!(store.load(&live.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn sweep_task_prunes_then_stops_on_shutdown() {
        let store = SweepableMemoryStore::new();
        let mut dead = record_expiring(-1);
        store.create(&mut dead).await.unwrap();
        assert_eq!(store.record_count().await, 1);

        let (tx, rx) = watch::channel(false);
        // A tiny period so the test doesn't wait an hour; the first immediate
        // tick is burned, so the first sweep lands ~one period in.
        let handle = spawn_session_sweep(store.clone(), Duration::from_millis(20), rx);

        // Wait until the sweep has pruned the expired record.
        for _ in 0..100 {
            if store.record_count().await == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            store.record_count().await,
            0,
            "the sweep task pruned the expired record"
        );

        // Shutdown stops the task.
        tx.send(true).unwrap();
        let joined = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(joined.is_ok(), "the sweep task must exit on shutdown");
    }

    // ── ACME lifecycle → status mapping (O1) ──────────────────────────────

    #[test]
    fn acme_status_cert_deployed_is_info_naming_domains() {
        let status = acme_status_for_event(
            &AcmeStatusEvent::CertDeployed,
            &["dux.example.com".to_string(), "api.example.com".to_string()],
        )
        .expect("a cert deploy must produce a status");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("acquired/renewed"),
            "must read as a lifecycle milestone: {}",
            status.message
        );
        assert!(
            status.message.contains("dux.example.com")
                && status.message.contains("api.example.com"),
            "must name the covered domains: {}",
            status.message
        );
    }

    #[test]
    fn acme_status_cache_store_is_silent() {
        // Internal cache bookkeeping is logged, not surfaced on the status line.
        assert!(
            acme_status_for_event(
                &AcmeStatusEvent::CacheStore,
                &["dux.example.com".to_string()]
            )
            .is_none(),
            "a cache-store event must not produce a user-facing status"
        );
    }

    #[test]
    fn acme_status_error_is_error_tone_with_retry_note() {
        let status = acme_status_for_event(
            &AcmeStatusEvent::Error("order: bad auth".to_string()),
            &["dux.example.com".to_string()],
        )
        .expect("an error must produce a status");
        assert_eq!(status.tone, "error");
        assert!(
            status.message.contains("order: bad auth"),
            "must carry the cause: {}",
            status.message
        );
        assert!(
            status.message.contains("retry") && status.message.contains("dux.log"),
            "must say renewal retries and point at dux.log: {}",
            status.message
        );
    }

    #[test]
    fn acme_status_stream_ended_is_error_tone() {
        let status = acme_status_for_event(&AcmeStatusEvent::StreamEnded, &[])
            .expect("stream-end must produce a status");
        assert_eq!(status.tone, "error");
        assert!(
            status.message.to_lowercase().contains("no longer renew"),
            "must warn renewal has stopped: {}",
            status.message
        );
    }

    #[test]
    fn classify_event_ok_maps_deploy_vs_cache() {
        use rustls_acme::EventOk;
        assert_eq!(
            classify_event_ok(&EventOk::DeployedNewCert),
            AcmeStatusEvent::CertDeployed
        );
        assert_eq!(
            classify_event_ok(&EventOk::DeployedCachedCert),
            AcmeStatusEvent::CertDeployed
        );
        assert_eq!(
            classify_event_ok(&EventOk::CertCacheStore),
            AcmeStatusEvent::CacheStore
        );
        assert_eq!(
            classify_event_ok(&EventOk::AccountCacheStore),
            AcmeStatusEvent::CacheStore
        );
    }

    #[test]
    fn acme_statuses_carry_the_acme_key() {
        let domains = vec!["example.com".to_string()];
        let deployed = acme_status_for_event(&AcmeStatusEvent::CertDeployed, &domains).unwrap();
        assert_eq!(deployed.key.as_deref(), Some("acme"));
        let err = acme_status_for_event(&AcmeStatusEvent::Error("x".into()), &domains).unwrap();
        assert_eq!(err.key.as_deref(), Some("acme"));
    }

    // ── HSTS ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn hsts_layer_stamps_the_header_on_every_response() {
        use tower::ServiceExt; // for `oneshot`
        let app = hsts_layer(Router::new().route("/", axum::routing::get(|| async { "ok" })));
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let hsts = resp
            .headers()
            .get(axum::http::header::STRICT_TRANSPORT_SECURITY)
            .and_then(|h| h.to_str().ok())
            .expect("hsts_layer must stamp the header");
        assert_eq!(hsts, HSTS_HEADER_VALUE);
        // The value is a 2-year max-age, includeSubDomains, and NO preload.
        assert!(hsts.contains("max-age=63072000"));
        assert!(hsts.contains("includeSubDomains"));
        assert!(!hsts.contains("preload"), "must NOT assert preload: {hsts}");
    }

    // ── :80 challenge/redirect router access log (F2) ───────────────────────

    /// Build a `:80` challenge/redirect router against a throwaway staging ACME
    /// state, wiring the given captured console + access-log toggle. The state is
    /// never contacted (no network in tests); it only needs to exist so the
    /// challenge route_service can mount.
    fn challenge_router_with_console(
        console: crate::console::Console,
        access_log: bool,
    ) -> (Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let plan = AcmePlan {
            http_addr: "0.0.0.0:80".parse().unwrap(),
            https_addr: "0.0.0.0:443".parse().unwrap(),
            domains: vec!["dux.example.com".to_string()],
            email: String::new(),
            production: false,
            cache_dir: tmp.path().join("acme"),
        };
        let (state, domains) = build_acme_state(&plan).expect("build acme state");
        let router = build_http_challenge_router(&state, 443, domains, console, access_log);
        (router, tmp)
    }

    #[tokio::test]
    async fn challenge_router_access_log_records_the_308_redirect() {
        use tower::ServiceExt; // for `oneshot`
        let (console, sink) = crate::console::Console::test_capture(false);
        let (router, _tmp) = challenge_router_with_console(console, true);
        // A plain request to an allowed Host falls through to the redirect fallback
        // → 308 to HTTPS. The access log must record that final status.
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/some/path")
                    .header(axum::http::header::HOST, "dux.example.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        let out = sink.contents();
        assert!(
            out.contains("/some/path") && out.contains("308"),
            "the :80 redirect must log a 308 access line: {out}"
        );
    }

    #[tokio::test]
    async fn challenge_router_access_log_toggle_off_emits_nothing() {
        use tower::ServiceExt; // for `oneshot`
        let (console, sink) = crate::console::Console::test_capture(false);
        let (router, _tmp) = challenge_router_with_console(console, false);
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/some/path")
                    .header(axum::http::header::HOST, "dux.example.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        assert!(
            sink.contents().is_empty(),
            "access_log = false must emit no :80 access lines: {}",
            sink.contents()
        );
    }

    #[tokio::test]
    async fn challenge_router_access_log_records_a_421_foreign_host() {
        use tower::ServiceExt; // for `oneshot`
        let (console, sink) = crate::console::Console::test_capture(false);
        let (router, _tmp) = challenge_router_with_console(console, true);
        // A foreign Host on a non-challenge path is 421'd by the redirect
        // sub-router's allowlist; the access log (outermost) still records it.
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/some/path")
                    .header(axum::http::header::HOST, "evil.example.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::MISDIRECTED_REQUEST);
        let out = sink.contents();
        assert!(
            out.contains("/some/path") && out.contains("421"),
            "a foreign-Host 421 on :80 must be access-logged: {out}"
        );
    }

    #[tokio::test]
    async fn access_log_strips_the_query_string() {
        use tower::ServiceExt; // for `oneshot`
        let (console, sink) = crate::console::Console::test_capture(false);
        let (router, _tmp) = challenge_router_with_console(console, true);
        // A request whose query carries a (would-be) session id and file path must
        // be logged by PATH ONLY — the query string must never reach the access log.
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/some/path?session_id=topsecret&path=/etc/shadow")
                    .header(axum::http::header::HOST, "dux.example.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        let out = sink.contents();
        assert!(out.contains("/some/path"), "the path must be logged: {out}");
        assert!(
            !out.contains("session_id") && !out.contains("topsecret") && !out.contains("shadow"),
            "the query string (session id, file path) must NOT be logged: {out}"
        );
    }

    #[tokio::test]
    async fn host_allowlist_missing_host_is_400_not_421() {
        use tower::ServiceExt; // for `oneshot`
        let (console, _sink) = crate::console::Console::test_capture(false);
        let (router, _tmp) = challenge_router_with_console(console, false);
        // A request with NO Host header is malformed (not misrouted): the
        // allowlist must answer 400 Bad Request, not 421 Misdirected Request.
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/some/path")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── build_acme_state cache-dir hardening ──────────────────────────────

    #[test]
    fn build_acme_state_creates_cache_dir_0700() {
        // The cache dir holds private keys; build_acme_state must create it and
        // tighten it to owner-only (0700). Use a deliberately loose pre-existing
        // dir to prove it is RE-tightened, not just created.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("acme-keys");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755)).unwrap();

        let plan = AcmePlan {
            http_addr: "0.0.0.0:80".parse().unwrap(),
            https_addr: "0.0.0.0:443".parse().unwrap(),
            domains: vec!["dux.example.com".to_string()],
            email: "ops@example.com".to_string(),
            production: false,
            cache_dir: cache.clone(),
        };
        let (_state, domains) = build_acme_state(&plan).expect("build acme state");
        assert_eq!(domains, vec!["dux.example.com".to_string()]);

        let mode = std::fs::metadata(&cache).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "the ACME cache dir must be owner-only (0700)");
    }

    #[test]
    fn build_acme_state_rejects_all_blank_domains() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = AcmePlan {
            http_addr: "0.0.0.0:80".parse().unwrap(),
            https_addr: "0.0.0.0:443".parse().unwrap(),
            domains: vec!["".to_string(), "   ".to_string()],
            email: String::new(),
            production: false,
            cache_dir: tmp.path().join("acme"),
        };
        assert!(
            build_acme_state(&plan).is_err(),
            "an all-blank domain list must be refused"
        );
    }
}
