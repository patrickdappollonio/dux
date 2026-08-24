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
//! 5. **`[server] tailscale` is not `"no"` and the Host is a literal IP inside
//!    Tailscale's own ranges** (CGNAT `100.64.0.0/10` or the `fd7a:115c:a1e0::/48`
//!    ULA). This is what makes the `auto` mode usable: the Tailscale listener
//!    comes and goes with the interface, and the router (with this allowlist
//!    inside it) is built once per serve, so a rule derived from what happened
//!    to be bound at that moment would 403 every tailnet device whenever dux
//!    re-bound the leg later. The rule is therefore STRUCTURAL: it is evaluated
//!    on every listener including loopback, and it fires even while the Tailscale
//!    leg is unbound. That is harmless under dux's trust model, because it admits
//!    a Host value and nothing else: reaching a listener at all is still the
//!    operator's network's business.
//!
//!    The MODE itself is live. A serve threads its Tailscale-mode cell in through
//!    [`HostAllowlist::with_live_tailscale_literals`], so changing
//!    `[server] tailscale` while dux serves moves this rule with the listener
//!    instead of leaving `no` admitting tailnet literals until a restart.
//!
//! A Tailscale MagicDNS name (`box.tailnet.ts.net`) is NOT an IP literal, so
//! rule 5 does not cover it and it still only works when the user adds it to
//! `[server] allowed_hosts`. Widening literals is safe where widening names is
//! not: DNS rebinding needs an attacker-controlled NAME, and no browser can be
//! made to send an IP-literal Host for a name the attacker owns. A local reverse
//! proxy that forwards a spoofed Host is the operator's own configuration and is
//! out of scope here, exactly as it already is for rule 2.

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

/// Whether `ip` is one of Tailscale's own addresses: the CGNAT `100.64.0.0/10`
/// v4 range or the `fd7a:115c:a1e0::/48` v6 ULA. Reuses the SAME predicates the
/// address detector parses with, so the guard and the detector can never disagree
/// about what a Tailscale address is.
fn is_tailscale_range(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => dux_core::tailscale::is_tailscale_cgnat(v4),
        IpAddr::V6(v6) => dux_core::tailscale::is_tailscale_ipv6(v6),
    }
}

// ── HostAllowlist ──────────────────────────────────────────────────────────

/// The Host allowlist built from the server's bound IPs and the operator's
/// configured hostname list. Implements the five DNS-rebinding allow rules
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
    /// Whether an IP literal in Tailscale's own ranges is accepted, whether or
    /// not that leg is bound right now. Rule 5; see the module doc for why it is
    /// structural and why a serve reads it live.
    tailscale_literals: TailscaleLiterals,
}

/// Where rule 5's answer comes from: a value fixed at construction, or a cell the
/// serve loop writes when `[server] tailscale` changes while dux is serving.
///
/// A serve threads the live cell in so one mode change moves the guard with the
/// listener. Tests and callers with no serve behind them use the fixed form.
#[derive(Debug, Clone)]
enum TailscaleLiterals {
    Fixed(bool),
    Live(Arc<std::sync::atomic::AtomicBool>),
}

impl TailscaleLiterals {
    fn allowed(&self) -> bool {
        match self {
            Self::Fixed(value) => *value,
            Self::Live(cell) => cell.load(std::sync::atomic::Ordering::SeqCst),
        }
    }
}

impl HostAllowlist {
    /// Build an allowlist from the IPs the server bound to and the operator's
    /// `[server] allowed_hosts` list. `bound_ips` is typically derived from the
    /// bound listeners' local addresses; `configured` is the raw string list from
    /// config (port suffixes are stripped and entries are lowercased here).
    ///
    /// `tailscale_literals` comes from the serve mode (`[server] tailscale` not
    /// being `"no"`) rather than from what bound, because the Tailscale leg may
    /// be bound and unbound many times behind this one allowlist. A serve that
    /// can change that mode while running follows this call with
    /// [`Self::with_live_tailscale_literals`].
    pub fn new(bound_ips: &[IpAddr], configured: &[String], tailscale_literals: bool) -> Self {
        let has_unspecified = bound_ips.iter().any(|ip| ip.is_unspecified());
        let configured = configured
            .iter()
            .filter_map(|h| normalize_host_for_match(h))
            .collect();
        Self {
            bound_ips: bound_ips.to_vec(),
            has_unspecified,
            configured,
            tailscale_literals: TailscaleLiterals::Fixed(tailscale_literals),
        }
    }

    /// Read rule 5 from a live cell instead of the constructed value, so a
    /// `[server] tailscale` change applied while dux serves moves the Host guard
    /// with the listener rather than waiting for a restart.
    pub fn with_live_tailscale_literals(
        mut self,
        cell: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.tailscale_literals = TailscaleLiterals::Live(cell);
        self
    }

    /// Whether a raw `Host` header value is allowed by any of the five rules.
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
            if self.bound_ips.contains(&ip) {
                return true;
            }
            // Rule 5: a literal in Tailscale's own ranges, while this server is
            // willing to serve the tailnet at all. Deliberately independent of
            // whether that leg is bound at this instant.
            return self.tailscale_literals.allowed() && is_tailscale_range(ip);
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
/// `live_tailscale_literals` is the serve's Tailscale-mode cell when there is a
/// serve behind this router, so rule 5 follows a mode change that happens while
/// dux is serving; `None` pins rule 5 to `tailscale_literals`.
pub fn host_allowlist_layer(
    router: Router,
    bound_ips: Vec<IpAddr>,
    configured: Vec<String>,
    tailscale_literals: bool,
    live_tailscale_literals: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Router {
    let allowlist = HostAllowlist::new(&bound_ips, &configured, tailscale_literals);
    let allowlist = Arc::new(match live_tailscale_literals {
        Some(cell) => allowlist.with_live_tailscale_literals(cell),
        None => allowlist,
    });
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
        let al = HostAllowlist::new(&[], &[], false);
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
        let al = HostAllowlist::new(&ips(&["100.64.0.1", "10.0.0.5"]), &[], false);
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
        let al = HostAllowlist::new(&[], &["box.tailnet.ts.net".to_string()], false);
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
        let al = HostAllowlist::new(&ips(&["0.0.0.0"]), &[], false);
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
        let al6 = HostAllowlist::new(&ips(&["::"]), &[], false);
        assert!(al6.allows_host("192.168.1.5"), "lan ip via :: bind");
    }

    /// When NO bound IP is unspecified, an arbitrary LAN IP that is NOT in
    /// `bound_ips` is rejected (rule 2 does not fire, rule 3 does not match).
    #[test]
    fn non_unspecified_bind_does_not_accept_arbitrary_ip() {
        // Bound to 127.0.0.1 only.
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], false);
        // Loopback still passes (rule 1), but a foreign IP is rejected.
        assert!(al.allows_host("127.0.0.1"), "loopback bound ip");
        assert!(!al.allows_host("192.168.1.5"), "arbitrary lan ip rejected");
        assert!(!al.allows_host("10.0.0.1"), "another lan ip rejected");
    }

    /// Unknown hostnames (neither loopback, nor bound IP, nor configured) are
    /// rejected.
    #[test]
    fn unknown_hostname_rejected() {
        let al = HostAllowlist::new(
            &ips(&["127.0.0.1"]),
            &["good.example.com".to_string()],
            false,
        );
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
        let al = HostAllowlist::new(&[], &["*".to_string()], false);
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

    // ── Rule 5: Tailscale-range IP literals ────────────────────────────────

    /// The laptop-roam case this rule exists for: the Tailscale leg is not bound
    /// right now (dux is loopback-only because the interface is away), a tailnet
    /// device's request arrives with a `100.x` Host, and it must not be a 403.
    /// The rule is structural, so it fires whether or not the leg happens to be
    /// up at this instant.
    #[test]
    fn a_tailscale_cgnat_literal_is_allowed_while_the_leg_is_unbound() {
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], true);
        assert!(
            al.allows_host("100.101.102.103"),
            "a CGNAT literal must pass even though only loopback is bound"
        );
        assert!(al.allows_host("100.101.102.103:8080"), "with a port");
        // The range boundaries, so the rule is the same 100.64.0.0/10 the
        // detector uses and not a looser "starts with 100".
        assert!(al.allows_host("100.64.0.0"), "first address in range");
        assert!(al.allows_host("100.127.255.255"), "last address in range");
        assert!(!al.allows_host("100.63.255.255"), "just below the range");
        assert!(!al.allows_host("100.128.0.0"), "just above the range");
    }

    /// The IPv6 half, including the bracketed form a browser actually sends.
    #[test]
    fn a_tailscale_ula_literal_is_allowed_bracketed_or_bare() {
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], true);
        assert!(al.allows_host("[fd7a:115c:a1e0::1234]"), "bracketed");
        assert!(al.allows_host("[fd7a:115c:a1e0::1234]:8080"), "with a port");
        // One past the /48, and a plain ULA, are not Tailscale addresses.
        assert!(!al.allows_host("[fd7a:115c:a1e1::1]"), "outside the /48");
        assert!(!al.allows_host("[fc00::1]"), "an ordinary ULA");
    }

    /// The rule follows a LIVE mode change: switching `[server] tailscale` while
    /// dux serves must move the Host guard with it, or `no` keeps admitting
    /// tailnet literals and `auto` keeps refusing them until a restart.
    #[test]
    fn a_live_flag_moves_the_tailscale_literal_rule_while_the_server_serves() {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], false)
            .with_live_tailscale_literals(Arc::clone(&flag));
        assert!(
            !al.allows_host("100.101.102.103"),
            "the mode is no, so a tailnet literal is refused"
        );
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            al.allows_host("100.101.102.103"),
            "the same allowlist must admit it once the mode wants Tailscale"
        );
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(
            !al.allows_host("100.101.102.103"),
            "and refuse it again on the way back"
        );
        // Every other rule is untouched by the live flag.
        assert!(al.allows_host("127.0.0.1"), "loopback is always allowed");
        assert!(!al.allows_host("box.tailnet.ts.net"), "names are unaffected");
    }

    /// The rule is off when the mode is `no`: a deployment that told dux to stay
    /// off the tailnet does not get a tailnet-shaped exemption.
    #[test]
    fn tailscale_literals_are_refused_when_the_mode_is_no() {
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], false);
        assert!(!al.allows_host("100.101.102.103"));
        assert!(!al.allows_host("[fd7a:115c:a1e0::1234]"));
    }

    /// The rule widens IP literals only. Names are still the DNS-rebinding
    /// surface, and a MagicDNS name still needs `allowed_hosts`.
    #[test]
    fn the_tailscale_rule_does_not_admit_any_hostname() {
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], true);
        assert!(!al.allows_host("box.tailnet.ts.net"), "magicdns name");
        assert!(!al.allows_host("evil.example.com"), "unknown name");
        assert!(!al.allows_host("192.168.1.5"), "an unrelated LAN literal");
    }

    /// A Host with a trailing dot (FQDN notation) is normalized before comparison.
    #[test]
    fn trailing_dot_normalized() {
        let al = HostAllowlist::new(&[], &["dux.example.com".to_string()], false);
        assert!(al.allows_host("dux.example.com."), "trailing dot stripped");
    }
}
