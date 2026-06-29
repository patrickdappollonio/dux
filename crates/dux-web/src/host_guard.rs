//! Host-allowlist middleware (DNS-rebinding defense) and the session-sweep
//! helpers used by the auth layer.
//!
//! ## What this module owns
//!
//! - [`DomainAllowlist`] + [`host_allowlist_layer`] -- the Host header guard
//!   that pins requests to configured domains so a DNS-rebinding attacker gets
//!   421 instead of a response.
//! - [`SweepableMemoryStore`] + [`spawn_session_sweep`] -- the bounded
//!   in-memory session store whose periodic sweep prunes expired records so a
//!   long-lived server does not accumulate dead session entries.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::Mutex;
use tokio::sync::watch;
use tower_sessions::cookie::time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, ExpiredDeletion, SessionStore};

// ── Host allowlist ─────────────────────────────────────────────────────────

/// Strip a `:port` from a (trimmed, non-empty) `Host` value, bracket-aware for
/// IPv6, returning the bare host (IPv6 kept bracketed).
///
/// - bracketed IPv6 (`[::1]` / `[::1]:80`) -- the bracketed literal, port dropped;
///   a missing closing bracket is malformed (`None`).
/// - bare host / IPv4 with a trailing all-digit `:port` -- the host, port dropped.
/// - an unbracketed multi-colon value (an unbracketed IPv6) is malformed (`None`).
/// - anything else -- unchanged.
fn strip_host_port(host: &str) -> Option<String> {
    if let Some(rest) = host.strip_prefix('[') {
        let close = rest.find(']')?;
        Some(format!("[{}]", &rest[..close]))
    } else {
        match host.rsplit_once(':') {
            Some((left, right))
                if right.chars().all(|c| c.is_ascii_digit()) && !right.is_empty() =>
            {
                if left.contains(':') {
                    return None; // unbracketed IPv6 with port -- malformed
                }
                Some(left.to_string())
            }
            Some((left, _)) if left.contains(':') => None, // unbracketed IPv6
            _ => Some(host.to_string()),
        }
    }
}

/// Normalize an incoming `Host` header for allowlist comparison: strip a `:port`
/// (bracket-aware for IPv6, via the shared [`strip_host_port`]), drop a single
/// trailing dot, lowercase.
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

/// The set of Host values a request may carry. Built from normalized domains
/// (e.g. from the configured domain list), so the accepted Hosts can never
/// drift from what the operator configured.
///
/// Module-private: it is an implementation detail of [`host_allowlist_layer`];
/// callers wire the allowlist through that function.
#[derive(Debug, Clone)]
struct DomainAllowlist {
    domains: Vec<String>,
}

impl DomainAllowlist {
    /// `domains` MUST already be normalized (lowercased, no trailing dot, no port).
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

/// Middleware: pin requests to the configured domains. A request whose `Host` is
/// present but not in the allowlist gets `421 Misdirected Request` (DNS-rebinding
/// defense). A request with NO usable `Host` header gets `400 Bad Request`.
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

/// Wrap a router with the Host allowlist middleware. Every route in the router
/// is pinned to the configured domains (DNS-rebinding defense).
pub fn host_allowlist_layer(router: Router, domains: Vec<String>) -> Router {
    let allowlist = Arc::new(DomainAllowlist::new(domains));
    router.layer(axum::middleware::from_fn_with_state(
        allowlist,
        host_allowlist_middleware,
    ))
}

// ── Session sweep ──────────────────────────────────────────────────────────

/// An in-memory session store that the periodic sweep can prune.
///
/// `tower_sessions::MemoryStore` keeps an `Arc<Mutex<HashMap<Id, Record>>>` whose
/// `load` already treats expired records as absent -- but it NEVER removes them,
/// so a long-lived server with login churn accumulates dead entries forever. That
/// store does not implement `ExpiredDeletion` and hides its map, so there is no
/// way to bolt a sweep onto it. This is the smallest correct fix: a byte-for-byte
/// reimplementation of `MemoryStore`'s `SessionStore` logic (the documented
/// reference impl) over a map WE own, plus an `ExpiredDeletion::delete_expired`
/// that evicts every record whose `expiry_date` has passed. No session crypto is
/// reimplemented -- signing, cookie handling, and id rotation stay in
/// `tower_sessions`'s `SessionManagerLayer`; this only owns the storage map.
#[derive(Clone, Debug, Default)]
pub struct SweepableMemoryStore(Arc<Mutex<HashMap<Id, Record>>>);

impl SweepableMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current record count -- test-only visibility into the sweep effect.
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_host_port ───────────────────────────────────────────────────

    #[test]
    fn strip_host_port_handles_bare_ipv6_and_brackets() {
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
        assert_eq!(strip_host_port("[::1]"), Some("[::1]".to_string()));
        assert_eq!(
            strip_host_port("[2001:db8::1]:80"),
            Some("[2001:db8::1]".to_string())
        );
        assert_eq!(strip_host_port("2001:db8::1"), None);
        assert_eq!(strip_host_port("2001:db8::1:443"), None);
        assert_eq!(strip_host_port("[::1"), None);
    }

    // ── DomainAllowlist ────────────────────────────────────────────────────

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

        assert!(store.load(&live.id).await.unwrap().is_some());
        assert!(store.load(&dead.id).await.unwrap().is_none());
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
        let handle = spawn_session_sweep(store.clone(), Duration::from_millis(20), rx);

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

        tx.send(true).unwrap();
        let joined = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(joined.is_ok(), "the sweep task must exit on shutdown");
    }
}
