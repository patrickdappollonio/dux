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
//! acceptor is the injectable seam: production serves the `:443` app via
//! [`serve_https_acme`] with rustls-acme's `AxumAcceptor`; the e2e drives the
//! IDENTICAL axum-server serving construction
//! (`bind/from_tcp(addr).handle(h).acceptor(acc).serve(make_svc)`) with a plain
//! self-signed `RustlsAcceptor`, exercising connect-info, graceful shutdown, and
//! WS upgrade over TLS. See `tests/tls_serving.rs`.

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
use futures_util::StreamExt;
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
pub fn normalize_domain(raw: &str) -> Result<Option<String>, String> {
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

/// Spawn the dedicated task that drives the `AcmeState` stream. rustls-acme does
/// NO background work itself — certificate acquisition and renewal only advance
/// while this stream is polled. Per the explicit-failure tenet every event is
/// logged: `Ok` at info (acquired/renewed/cached), `Err` at error (loud, so a
/// failing renewal is visible in `dux.log`), and a stream END is an error too
/// because it means certificates will silently stop renewing.
///
/// Returns the `JoinHandle` so the serve path can abort it on shutdown.
pub fn spawn_acme_event_task(mut state: DuxAcmeState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match state.next().await {
                Some(Ok(ok)) => {
                    dux_core::logger::info(&format!(
                        "[acme] certificate lifecycle event: {ok:?} — TLS certificates are \
                         being acquired/renewed via Let's Encrypt."
                    ));
                }
                Some(Err(err)) => {
                    dux_core::logger::error(&format!(
                        "[acme] certificate lifecycle ERROR: {err} — TLS may be using a stale or \
                         missing certificate. Check that :80 is reachable from the internet for \
                         HTTP-01 validation and that the domain's DNS points here."
                    ));
                }
                None => {
                    dux_core::logger::error(
                        "[acme] the certificate lifecycle stream ended — certificates will NOT \
                         renew. The HTTPS server keeps serving the last certificate until it \
                         expires; restart dux to recover automatic renewal.",
                    );
                    break;
                }
            }
        }
    })
}

// ── :80 challenge + redirect router ────────────────────────────────────────

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
pub fn redirect_target(host_header: Option<&str>, https_port: u16, uri: &Uri) -> Option<String> {
    let host = host_header?.trim();
    if host.is_empty() {
        return None;
    }
    // Strip any :port from the Host. IPv6 literals are bracketed (`[::1]` or
    // `[::1]:80`); split the port off AFTER the closing bracket. A bare host with
    // a single colon and digits after it is `host:port`.
    let host_no_port = if let Some(rest) = host.strip_prefix('[') {
        // Bracketed IPv6: keep through the closing bracket, drop a trailing :port.
        let close = rest.find(']')?;
        // Re-add the brackets; ignore whatever follows the bracket (an optional
        // :port).
        format!("[{}]", &rest[..close])
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
                left.to_string()
            }
            Some((left, _)) if left.contains(':') => return None, // unbracketed IPv6
            _ => host.to_string(),
        }
    };
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
/// token-keyed, returns 404 for unknown tokens, and the CA dials it by IP) and a
/// fallback that redirects everything else to HTTPS. The allowlist middleware is
/// layered onto the FALLBACK only (the challenge route is a `route_service`, so
/// the fallback layer never touches it).
pub fn build_http_challenge_router(
    state: &DuxAcmeState,
    https_port: u16,
    domains: Vec<String>,
) -> Router {
    let challenge = state.http01_challenge_tower_service();
    let allowlist = Arc::new(DomainAllowlist::new(domains));
    Router::new()
        .route_service("/.well-known/acme-challenge/{token}", challenge)
        .fallback(redirect_to_https)
        // The allowlist guards the redirect fallback (a rebinding attacker must
        // not get a redirect that legitimizes their hostname). `route_service`
        // routes are matched before the fallback and are NOT wrapped by a
        // fallback-scoped layer, so the challenge stays exempt.
        .layer(axum::middleware::from_fn_with_state(
            allowlist,
            host_allowlist_middleware,
        ))
        .with_state(RedirectState { https_port })
}

// ── Host allowlist (deferral 2) ────────────────────────────────────────────

/// The set of Host values a request may carry when dux terminates TLS. Built from
/// the SAME normalized domains used for issuance, so the cert names and the
/// accepted Hosts can never drift.
#[derive(Debug, Clone)]
pub struct DomainAllowlist {
    domains: Vec<String>,
}

impl DomainAllowlist {
    /// `domains` MUST already be normalized (via [`normalize_domains`]).
    pub fn new(domains: Vec<String>) -> Self {
        Self { domains }
    }

    /// Whether a raw `Host` header value is allowed: strip any `:port`, drop a
    /// trailing dot, lowercase, then membership-test against the normalized set.
    pub fn allows(&self, host_header: &str) -> bool {
        let Some(host) = normalize_host_for_match(host_header) else {
            return false;
        };
        self.domains.contains(&host)
    }
}

/// Normalize an incoming `Host` header for allowlist comparison: strip a `:port`
/// (bracket-aware for IPv6), drop a single trailing dot, lowercase. Mirrors the
/// host-stripping in [`redirect_target`] so the two paths agree.
fn normalize_host_for_match(host_header: &str) -> Option<String> {
    let host = host_header.trim();
    if host.is_empty() {
        return None;
    }
    let host_no_port = if let Some(rest) = host.strip_prefix('[') {
        let close = rest.find(']')?;
        format!("[{}]", &rest[..close])
    } else {
        match host.rsplit_once(':') {
            Some((left, right))
                if right.chars().all(|c| c.is_ascii_digit()) && !right.is_empty() =>
            {
                if left.contains(':') {
                    return None;
                }
                left.to_string()
            }
            Some((left, _)) if left.contains(':') => return None,
            _ => host.to_string(),
        }
    };
    let lowered = host_no_port.to_ascii_lowercase();
    let no_dot = lowered.strip_suffix('.').unwrap_or(&lowered);
    if no_dot.is_empty() {
        None
    } else {
        Some(no_dot.to_string())
    }
}

/// Middleware: reject any request whose `Host` is not in the allowlist with
/// `421 Misdirected Request` (the semantically correct code for "this server is
/// not authoritative for the requested host"). DNS-rebinding defense: an attacker
/// who points a controlled hostname at this IP gets a 421 instead of a response
/// that would let their page talk to dux.
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
        _ => (
            StatusCode::MISDIRECTED_REQUEST,
            "this dux server does not serve the requested host",
        )
            .into_response(),
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
/// The acceptor is the seam that makes this testable: `serve_app_over_axum`
/// below is the SHARED, acceptor-generic serve path the e2e drives with a plain
/// self-signed `RustlsAcceptor`. Production passes rustls-acme's `AxumAcceptor`;
/// neither path has test-only serving code.
pub async fn serve_https_acme(
    addr: SocketAddr,
    app: Router,
    acceptor: rustls_acme::axum::AxumAcceptor,
    handle: axum_server::Handle<SocketAddr>,
) -> std::io::Result<()> {
    axum_server::bind(addr)
        .handle(handle)
        .acceptor(acceptor)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
}

/// Serve the plain `:80` challenge + redirect router with a graceful-shutdown
/// handle and connect-info (kept for parity even though the redirect path does
/// not read the peer). Returns the accept loop's `io::Result`.
pub async fn serve_http_challenge(
    addr: SocketAddr,
    app: Router,
    handle: axum_server::Handle<SocketAddr>,
) -> std::io::Result<()> {
    axum_server::bind(addr)
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
