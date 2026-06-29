//! Host-allowlist middleware (DNS-rebinding defense).
//!
//! ## What this module owns
//!
//! - [`HostAllowlist`] + [`host_allowlist_layer`] -- the Host header guard that
//!   pins requests to the server's own bound addresses and any operator-configured
//!   hostnames, so a DNS-rebinding attacker gets 403 instead of a response.
//!
//! ## Allow rules (NO wildcard)
//!
//! Given `bound_ips` (the IPs the server actually bound to) and `configured`
//! (the `[server] allowed_hosts` list), a Host is allowed when:
//!
//! 1. It is a loopback literal (`localhost`, `127.0.0.1`, `[::1]`, or any IP
//!    that `is_loopback()`).
//! 2. **Any `bound_ips` entry is unspecified (`0.0.0.0` / `::`): accept any
//!    Host that parses as an `IpAddr`.** A `0.0.0.0` bind is reachable at every
//!    local IP (e.g. `192.168.1.5`); pinning to the literal `0.0.0.0` would 403
//!    all real LAN clients. Safe: a DNS-rebinding attacker cannot make a browser
//!    send an IP-literal Host for a hostname they control.
//! 3. The Host parses as an `IpAddr` that is in `bound_ips` (covers Tailscale
//!    `100.x` literals and any explicit `--bind` IP).
//! 4. The Host case-insensitively equals a (port-stripped) entry in `configured`.
//!
//! A Tailscale MagicDNS name (`box.tailnet.ts.net`) is NOT an IP literal and is
//! NOT in `bound_ips` by default, so it only works when the user adds it to
//! `[server] allowed_hosts`. The `100.x` Tailscale IP works without configuration
//! via rule 3.

use std::net::IpAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

// ── Host normalization helpers ─────────────────────────────────────────────

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

/// Parse a normalized (port-stripped, lowercased) host string as an `IpAddr`,
/// handling both plain IPv4/IPv6 and bracketed IPv6 (`[::1]`).
fn parse_normalized_host_as_ip(host: &str) -> Option<IpAddr> {
    // Plain IPv4 or bare IPv6 (the latter is malformed in Host but may appear in
    // configured hosts; parse defensively).
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip);
    }
    // Bracketed IPv6 as it appears after normalize_host_for_match.
    if let Some(inner) = host.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
        && let Ok(ip) = inner.parse::<IpAddr>()
    {
        return Some(ip);
    }
    None
}

// ── HostAllowlist ──────────────────────────────────────────────────────────

/// The Host allowlist built from the server's bound IPs and the operator's
/// configured hostname list. Implements the four DNS-rebinding allow rules
/// described in the module doc. Thread-safe via interior immutability: clone the
/// `Arc` for each request, never mutate after construction.
///
/// Construct with [`HostAllowlist::new`] and test with [`HostAllowlist::allows_host`].
#[derive(Debug, Clone)]
pub struct HostAllowlist {
    /// The raw bound IPs (for rule 3 membership test). Loopback IPs here are
    /// redundant (rule 1 covers them) but harmless.
    bound_ips: Vec<IpAddr>,
    /// True when any `bound_ips` entry is unspecified (`0.0.0.0` or `::`).
    /// Cached at construction; tested per-request by rule 2.
    has_unspecified: bool,
    /// Operator-configured hostnames, already normalized (lowercased, port
    /// stripped, no trailing dot) so per-request comparison is a simple
    /// `contains`. Rule 4.
    configured: Vec<String>,
}

impl HostAllowlist {
    /// Build an allowlist from the IPs the server bound to and the operator's
    /// `[server] allowed_hosts` list. `bound_ips` is typically derived from the
    /// bound listeners' local addresses; `configured` is the raw string list from
    /// config (port suffixes are stripped and entries are lowercased here).
    pub fn new(bound_ips: &[IpAddr], configured: &[String]) -> Self {
        let has_unspecified = bound_ips.iter().any(|ip| ip.is_unspecified());
        let configured = configured
            .iter()
            .filter_map(|h| normalize_host_for_match(h))
            .collect();
        Self {
            bound_ips: bound_ips.to_vec(),
            has_unspecified,
            configured,
        }
    }

    /// Whether a raw `Host` header value is allowed by any of the four rules.
    ///
    /// Normalizes the host (strip port, lowercase) before every comparison.
    /// A malformed or empty `Host` returns `false`.
    pub fn allows_host(&self, host_header: &str) -> bool {
        let Some(host) = normalize_host_for_match(host_header) else {
            return false;
        };

        // Rule 1: `localhost` (the non-IP alias); IP-valued loopbacks are
        // handled below after parsing.
        if host == "localhost" {
            return true;
        }

        // Try to parse the normalized host as an IP address (IPv4 or bracketed
        // IPv6). All four IP-valued rules go through this arm.
        if let Some(ip) = parse_normalized_host_as_ip(&host) {
            // Rule 1 (IP variant): any loopback IP (127.0.0.0/8, ::1).
            if ip.is_loopback() {
                return true;
            }
            // Rule 2: any bound IP is unspecified (0.0.0.0 / ::) -- accept any
            // IP literal. The caller intentionally exposed every local address.
            if self.has_unspecified {
                return true;
            }
            // Rule 3: the exact IP is one we bound to (e.g. the Tailscale 100.x).
            return self.bound_ips.contains(&ip);
        }

        // Rule 4: operator-configured hostname (case-insensitive, port-stripped).
        self.configured.contains(&host)
    }
}

// ── Middleware ─────────────────────────────────────────────────────────────

/// Middleware: reject requests whose `Host` is not in the allowlist.
/// A present-but-disallowed Host gets `403 Forbidden` (DNS-rebinding defense).
/// A missing or malformed Host also gets `403` (a well-formed HTTP/1.1 request
/// must carry a Host; an absent one is never legitimate here).
async fn host_allowlist_middleware(
    State(allowlist): State<Arc<HostAllowlist>>,
    request: Request,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok());
    match host {
        Some(h) if allowlist.allows_host(h) => next.run(request).await,
        Some(_) => (
            StatusCode::FORBIDDEN,
            "this dux server does not serve the requested host",
        )
            .into_response(),
        None => (StatusCode::FORBIDDEN, "missing or invalid Host header").into_response(),
    }
}

/// Wrap a router with the Host allowlist middleware. Every route in the router
/// is pinned to the allowed host set (DNS-rebinding defense). This layer should
/// sit OUTSIDE the access-log layer so rejected probes are not access-logged.
pub fn host_allowlist_layer(
    router: Router,
    bound_ips: Vec<IpAddr>,
    configured: Vec<String>,
) -> Router {
    let allowlist = Arc::new(HostAllowlist::new(&bound_ips, &configured));
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

    // ── HostAllowlist::allows_host ─────────────────────────────────────────

    fn ips(addrs: &[&str]) -> Vec<IpAddr> {
        addrs.iter().map(|s| s.parse().unwrap()).collect()
    }

    /// Rule 1: `localhost` and loopback IPs are ALWAYS allowed, regardless of the
    /// bound IP set.
    #[test]
    fn loopback_always_allowed() {
        // No bound IPs, no configured hosts -- still allows loopback.
        let al = HostAllowlist::new(&[], &[]);
        assert!(al.allows_host("localhost"), "localhost");
        assert!(al.allows_host("localhost:8080"), "localhost with port");
        assert!(al.allows_host("127.0.0.1"), "ipv4 loopback");
        assert!(al.allows_host("127.0.0.1:9000"), "ipv4 loopback with port");
        assert!(al.allows_host("[::1]"), "ipv6 loopback");
        assert!(al.allows_host("[::1]:8080"), "ipv6 loopback with port");
        // Whole loopback range (127.0.0.2 etc.) is allowed via ip.is_loopback().
        assert!(al.allows_host("127.0.0.2"), "other loopback IP");
    }

    /// Rule 3: an IP that exactly appears in `bound_ips` is allowed (covers the
    /// Tailscale 100.x literal and any explicit --bind address).
    #[test]
    fn bound_ip_literal_allowed() {
        let al = HostAllowlist::new(&ips(&["100.64.0.1", "10.0.0.5"]), &[]);
        assert!(al.allows_host("100.64.0.1"), "tailscale ip");
        assert!(al.allows_host("100.64.0.1:8080"), "tailscale ip with port");
        assert!(al.allows_host("10.0.0.5"), "lan ip");
        // An IP NOT in the set is rejected.
        assert!(!al.allows_host("10.0.0.6"), "different ip");
    }

    /// Rule 4: operator-configured hostnames are matched case-insensitively and
    /// port suffixes are stripped before comparison.
    #[test]
    fn configured_hostname_case_insensitive_with_and_without_port() {
        let al = HostAllowlist::new(&[], &["box.tailnet.ts.net".to_string()]);
        assert!(al.allows_host("box.tailnet.ts.net"), "exact match");
        assert!(al.allows_host("BOX.TAILNET.TS.NET"), "uppercase");
        assert!(al.allows_host("Box.Tailnet.Ts.Net"), "mixed case");
        assert!(al.allows_host("box.tailnet.ts.net:8080"), "with port");
        assert!(
            al.allows_host("BOX.tailnet.ts.net:443"),
            "mixed case with port"
        );
        // A different hostname is rejected.
        assert!(!al.allows_host("evil.tailnet.ts.net"), "different hostname");
    }

    /// Rule 2: when ANY bound IP is unspecified (0.0.0.0 or ::), accept any
    /// Host that parses as an IpAddr. This covers LAN IPs when the server binds
    /// to the wildcard address.
    #[test]
    fn unspecified_bind_accepts_any_ip_literal() {
        // 0.0.0.0 bind -- any IP literal allowed.
        let al = HostAllowlist::new(&ips(&["0.0.0.0"]), &[]);
        assert!(
            al.allows_host("192.168.1.5"),
            "lan ip allowed via 0.0.0.0 bind"
        );
        assert!(al.allows_host("10.0.0.1"), "another lan ip");
        assert!(al.allows_host("100.64.0.9"), "tailscale ip");
        // But a hostname is still NOT allowed (it's not an IP literal).
        assert!(
            !al.allows_host("evil.example.com"),
            "hostname rejected even with 0.0.0.0 bind"
        );

        // :: bind (IPv6 wildcard) -- same rule applies.
        let al6 = HostAllowlist::new(&ips(&["::"]), &[]);
        assert!(al6.allows_host("192.168.1.5"), "lan ip via :: bind");
    }

    /// When NO bound IP is unspecified, an arbitrary LAN IP that is NOT in
    /// `bound_ips` is rejected (rule 2 does not fire, rule 3 does not match).
    #[test]
    fn non_unspecified_bind_does_not_accept_arbitrary_ip() {
        // Bound to 127.0.0.1 only.
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[]);
        // Loopback still passes (rule 1), but a foreign IP is rejected.
        assert!(al.allows_host("127.0.0.1"), "loopback bound ip");
        assert!(!al.allows_host("192.168.1.5"), "arbitrary lan ip rejected");
        assert!(!al.allows_host("10.0.0.1"), "another lan ip rejected");
    }

    /// Unknown hostnames (neither loopback, nor bound IP, nor configured) are
    /// rejected.
    #[test]
    fn unknown_hostname_rejected() {
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &["good.example.com".to_string()]);
        assert!(!al.allows_host("evil.example.com"), "unknown hostname");
        assert!(
            !al.allows_host("good.example.com.evil.com"),
            "subdomain attack"
        );
        assert!(!al.allows_host(""), "empty host");
        assert!(!al.allows_host("   "), "whitespace host");
    }

    /// There is NO wildcard behavior: `"*"` in `configured` is treated as a
    /// literal string, not a glob. It does not grant access to arbitrary
    /// hostnames; only a Host header that contains the literal `"*"` would match
    /// it (which no browser or legitimate client sends).
    #[test]
    fn no_wildcard_behavior() {
        let al = HostAllowlist::new(&[], &["*".to_string()]);
        // `"*"` does not match any real hostname -- no wildcard expansion.
        assert!(
            !al.allows_host("anything.example.com"),
            "wildcard has no effect"
        );
        assert!(
            !al.allows_host("evil.example.com"),
            "another hostname rejected"
        );
        // Only the literal string `"*"` would match, which is not a real Host.
    }

    /// A Host with a trailing dot (FQDN notation) is normalized before comparison.
    #[test]
    fn trailing_dot_normalized() {
        let al = HostAllowlist::new(&[], &["dux.example.com".to_string()]);
        assert!(al.allows_host("dux.example.com."), "trailing dot stripped");
    }
}
