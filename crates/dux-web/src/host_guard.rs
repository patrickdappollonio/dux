//! Host-allowlist middleware (DNS-rebinding defense).
//!
//! ## What this module owns
//!
//! - [`DomainAllowlist`] + [`host_allowlist_layer`] -- the Host header guard
//!   that pins requests to configured domains so a DNS-rebinding attacker gets
//!   421 instead of a response.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

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
pub(crate) fn normalize_host_for_match(host_header: &str) -> Option<String> {
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
pub(crate) struct DomainAllowlist {
    domains: Vec<String>,
}

impl DomainAllowlist {
    /// `domains` MUST already be normalized (lowercased, no trailing dot, no port).
    pub(crate) fn new(domains: Vec<String>) -> Self {
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
}
