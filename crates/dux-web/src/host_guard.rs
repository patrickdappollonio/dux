//! Host-allowlist middleware (DNS-rebinding defense).
//!
//! [`HostAllowlist`] and [`host_allowlist_layer`] pin requests to the server's own
//! bound addresses, this machine's own tailnet name, and any operator-configured
//! hostnames, so a DNS-rebinding attacker gets a 403.
//!
//! Given `bound_ips`, the IPs the server actually bound to, and `configured`, the
//! `[server] allowed_hosts` list, a Host is allowed when (no wildcard):
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
//!    The list is live: a config reload rewrites it under the running router
//!    (see [`LiveHostNames`]), so editing `allowed_hosts` needs no restart.
//! 5. **`[server] tailscale` is not `"no"` and the Host is a literal IP inside
//!    Tailscale's own ranges** (CGNAT `100.64.0.0/10` or the `fd7a:115c:a1e0::/48`
//!    ULA). Structural rather than derived from what is bound: the leg comes and
//!    goes with the interface while the router is built once per serve, so this is
//!    evaluated on every listener and fires even while the leg is unbound. The MODE
//!    is live, threaded in through
//!    [`HostAllowlist::with_live_tailscale_literals`], so a mode change while dux
//!    serves moves this rule with the listener.
//! 6. **`[server] tailscale` is not `"no"` and the Host case-insensitively equals
//!    THIS machine's own MagicDNS name**, on any port, within the limits listed
//!    below. The name is what `tailscale status --json` reports for this
//!    machine, kept current by the Tailscale watcher on every mode but `no`, so a
//!    tailnet rename moves it with no restart; it rides
//!    the same live mode as rule 5, so `no` closes both. Any port, because the
//!    name reaches dux two ways: plain `http://<name>:<port>` over the Tailscale
//!    leg, and a `tailscale serve` route, which forwards the request with its
//!    Host (and its `https` Origin) unchanged, port included.
//!
//! Rule 6 is the one place this guard admits a NAME nobody typed into
//! `allowed_hosts`, so it is narrow on purpose, and every condition below is
//! enforced, not assumed:
//!
//! - One name, this machine's: every other `*.ts.net` (another machine on the
//!   same tailnet, a subdomain of this one, the tailnet suffix itself) is
//!   refused.
//! - Only a name Tailscale assigned: it must sit under the daemon's MagicDNS
//!   suffix, and that suffix under `.ts.net`. A name from another control server
//!   (Headscale, say) is in a domain its operator chose and needs
//!   `allowed_hosts`.
//! - Never while ANY Tailscale Funnel route is on for this machine and NO
//!   password is set, whatever the Funnel forwards to. This is not what keeps
//!   a Funnel's visitors out: Tailscale's serve proxy routes by the TLS name
//!   and forwards the public client's own Host, so a request through Funnel
//!   can claim `localhost` or a tailnet literal and never needs this name. It
//!   stops dux offering a name a Funnel is using while nothing would stop its
//!   visitors. With a password set the name stays admitted, so a Funnel's
//!   visitors reach the login page (`crate::auth`), which is where they are
//!   stopped.
//! - Only on a CURRENT reading: a failed look at the Tailscale CLI withdraws the
//!   name until a look succeeds. The name is watched on `yes` too.
//!
//! This guard refuses nothing because of a Funnel. What dux knows about one
//! (`crate::exposure`) decides how a request is CLASSIFIED instead: a request
//! carrying Tailscale's Funnel marker is the internet, and while a Funnel or a
//! raw TCP forward may reach dux's port (or nobody knows yet whether one
//! does), a loopback request with no trustworthy origin is the network, so a
//! configured password applies to it. With no password dux serves and says
//! loudly what that exposes; that choice is the owner's.
//!
//! Within those limits it is safe for the same reason the literal rules are: DNS
//! rebinding needs a name the ATTACKER controls, pointed at this machine's
//! address, and this name is assigned by the tailnet's administrator rather than
//! by anyone who can serve a web page. A local reverse proxy forwarding a spoofed
//! Host is the operator's own configuration, here as for rule 2.
//!
//! Widening literals is safe where widening names generally is not, for the
//! same reason: no browser can be made to send an IP-literal Host for a name the
//! attacker owns.

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
/// configured hostname list, implementing the allow rules in the module doc.
/// Thread-safe by interior immutability: clone the `Arc` per request, never mutate
/// after construction. Built with [`HostAllowlist::new`], asked with
/// [`HostAllowlist::allows_host`].
#[derive(Clone)]
pub struct HostAllowlist {
    /// The raw bound IPs (for rule 3 membership test). Loopback IPs here are
    /// redundant (rule 1 covers them) but harmless.
    bound_ips: Vec<IpAddr>,
    /// True when any `bound_ips` entry is unspecified (`0.0.0.0` or `::`).
    /// Cached at construction; tested per-request by rule 2.
    has_unspecified: bool,
    /// Operator-configured hostnames, normalized on the way in (lowercased,
    /// port stripped, no trailing dot) so per-request comparison is a simple
    /// `contains`. Rule 4. Live, so a config reload replaces the list in place.
    configured: LiveHostNames,
    /// Whether an IP literal in Tailscale's own ranges is accepted, whether or
    /// not that leg is bound right now. Rule 5; see the module doc for why it is
    /// structural and why a serve reads it live. Rule 6 is gated on it too.
    tailscale_literals: TailscaleLiterals,
    /// This machine's own MagicDNS name, at most one, written by the serve loop
    /// whenever the Tailscale watcher reads a different one. Rule 6.
    own_magicdns_name: LiveHostNames,
    /// What the serve knows about a Funnel, which withdraws rule 6's name while
    /// no password is set. `None` for a router no serve loop writes to.
    exposure: Option<crate::exposure::ExposureCell>,
    /// Whether a password is set right now. Read per request, so setting or
    /// clearing one moves rule 6 with no restart.
    password_set: Option<PasswordSet>,
    /// Whether the Host rules run at all. Off only for a router built with no
    /// bound address and no configured host.
    host_rules: bool,
}

/// Answers whether the web password is set right now.
pub type PasswordSet = Arc<dyn Fn() -> bool + Send + Sync>;

/// A set of Host names a running serve can replace in place: the configured
/// `allowed_hosts` (rule 4), which a config reload rewrites, and this machine's
/// own MagicDNS name (rule 6), which the Tailscale watcher rewrites when the
/// tailnet is renamed.
///
/// Names are normalized as they are stored, so a request pays one lowercase
/// comparison per entry and nothing else. Cloning shares the set: the clone the
/// guard holds and the clone the writer holds are the same names.
#[derive(Debug, Clone, Default)]
pub struct LiveHostNames(Arc<std::sync::RwLock<Vec<String>>>);

impl LiveHostNames {
    /// A set holding `raw`, normalized. An entry that does not normalize (an
    /// empty string, an unbracketed IPv6 with a port) is dropped.
    pub fn new(raw: &[String]) -> Self {
        let names = Self::default();
        names.replace(raw);
        names
    }

    /// Replace every name in the set.
    pub fn replace(&self, raw: &[String]) {
        let normalized: Vec<String> = raw
            .iter()
            .filter_map(|h| normalize_host_for_match(h))
            .collect();
        match self.0.write() {
            Ok(mut slot) => *slot = normalized,
            Err(poisoned) => *poisoned.into_inner() = normalized,
        }
    }

    /// The names as stored, normalized.
    pub fn snapshot(&self) -> Vec<String> {
        match self.0.read() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Whether `host`, ALREADY normalized, is in the set.
    fn contains(&self, host: &str) -> bool {
        match self.0.read() {
            Ok(slot) => slot.iter().any(|name| name == host),
            Err(poisoned) => poisoned.into_inner().iter().any(|name| name == host),
        }
    }
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
    /// Build an allowlist from the IPs the server bound to and the raw
    /// `[server] allowed_hosts` list, whose port suffixes are stripped and entries
    /// lowercased here. `tailscale_literals` comes from the serve mode rather than
    /// from what bound, because the leg may be bound and unbound many times behind
    /// one allowlist; a serve that can change the mode while running follows this
    /// with [`Self::with_live_tailscale_literals`].
    pub fn new(bound_ips: &[IpAddr], configured: &[String], tailscale_literals: bool) -> Self {
        let has_unspecified = bound_ips.iter().any(|ip| ip.is_unspecified());
        Self {
            bound_ips: bound_ips.to_vec(),
            has_unspecified,
            configured: LiveHostNames::new(configured),
            tailscale_literals: TailscaleLiterals::Fixed(tailscale_literals),
            own_magicdns_name: LiveHostNames::default(),
            exposure: None,
            password_set: None,
            host_rules: true,
        }
    }

    /// Skip the Host rules. For a router with no bound address to pin Hosts
    /// to; no serve path builds one.
    pub fn without_host_rules(mut self) -> Self {
        self.host_rules = false;
        self
    }

    /// Read whether a Funnel is on from the serve's exposure, and whether a
    /// password is set from `password_set`: together they decide whether rule
    /// 6's name is withdrawn.
    pub fn with_exposure(
        mut self,
        cell: crate::exposure::ExposureCell,
        password_set: PasswordSet,
    ) -> Self {
        self.exposure = Some(cell);
        self.password_set = Some(password_set);
        self
    }

    /// Whether rule 6's name is withdrawn right now: a Funnel is on for
    /// anything on this machine and no password would stop its visitors.
    fn name_withdrawn_for_funnel(&self) -> bool {
        let funnel = self
            .exposure
            .as_ref()
            .is_some_and(|cell| cell.get().funnel_any());
        funnel && !self.password_set.as_ref().is_some_and(|set| set())
    }

    /// Read rule 4 from a set a config reload rewrites, instead of the list
    /// given at construction.
    pub fn with_live_configured_hosts(mut self, names: LiveHostNames) -> Self {
        self.configured = names;
        self
    }

    /// Read rule 6 from the set the serve loop writes this machine's MagicDNS
    /// name into. Without it the guard admits no tailnet name of its own accord.
    pub fn with_live_own_magicdns_name(mut self, names: LiveHostNames) -> Self {
        self.own_magicdns_name = names;
        self
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

    /// Whether a raw `Host` header value is allowed by any of the six rules.
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
        if self.configured.contains(&host) {
            return true;
        }

        // Rule 6: this machine's own MagicDNS name, while the serve is willing to
        // serve the tailnet at all. Any port: dux's own port for plain HTTP, and
        // whatever port a `tailscale serve` route answers on, which forwards the
        // name unchanged.
        self.tailscale_literals.allowed()
            && self.own_magicdns_name.contains(&host)
            && !self.name_withdrawn_for_funnel()
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
    if !allowlist.host_rules {
        return next.run(request).await;
    }
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(crate::auth::provenance::header_text);
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

/// Wrap a router with the Host allowlist middleware, pinning every route to the
/// allowed host set. This layer must sit OUTSIDE the access-log layer, so rejected
/// probes are not access-logged. The caller builds the allowlist with whichever
/// live sets its serve keeps (the Tailscale mode, the configured hosts, this
/// machine's own MagicDNS name), so each rule follows its own writer.
pub fn host_allowlist_layer(router: Router, allowlist: HostAllowlist) -> Router {
    router.layer(axum::middleware::from_fn_with_state(
        Arc::new(allowlist),
        host_allowlist_middleware,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

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
        assert!(
            !al.allows_host("box.tailnet.ts.net"),
            "names are unaffected"
        );
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
    /// surface: rule 6 admits this machine's own name and nothing else, and
    /// without it a MagicDNS name needs `allowed_hosts`.
    #[test]
    fn the_tailscale_rule_does_not_admit_any_hostname() {
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], true);
        assert!(!al.allows_host("box.tailnet.ts.net"), "magicdns name");
        assert!(!al.allows_host("evil.example.com"), "unknown name");
        assert!(!al.allows_host("192.168.1.5"), "an unrelated LAN literal");
    }

    // ── Rule 6: this machine's own MagicDNS name ───────────────────────────

    /// An allowlist on a Tailscale-wanting serve whose own name is `own`.
    fn with_own_name(own: &str) -> (HostAllowlist, LiveHostNames, Arc<AtomicBool>) {
        let names = LiveHostNames::new(&[own.to_string()]);
        let mode = Arc::new(AtomicBool::new(true));
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], true)
            .with_live_tailscale_literals(Arc::clone(&mode))
            .with_live_own_magicdns_name(names.clone());
        (al, names, mode)
    }

    #[test]
    fn this_machines_own_magicdns_name_is_allowed_with_or_without_a_port() {
        let (al, _, _) = with_own_name("box.tail.ts.net");
        assert!(al.allows_host("box.tail.ts.net"));
        assert!(
            al.allows_host("box.tail.ts.net:3890"),
            "plain http on dux's port"
        );
        assert!(
            al.allows_host("box.tail.ts.net:8443"),
            "a tailscale serve port"
        );
        assert!(al.allows_host("BOX.Tail.TS.NET"), "case-insensitive");
        assert!(al.allows_host("box.tail.ts.net."), "trailing dot");
    }

    /// A Funnel withdraws the name only while no password would stop its
    /// visitors: with one set they must reach the login page, which needs the
    /// name.
    #[test]
    fn a_funnel_withdraws_the_name_only_while_no_password_is_set() {
        let (al, _, _) = with_own_name("box.tail.ts.net");
        let exposure = crate::exposure::ExposureCell::new(crate::exposure::FunnelState::Open);
        let password = Arc::new(AtomicBool::new(false));
        let reads = Arc::clone(&password);
        let al = al.with_exposure(
            exposure.clone(),
            Arc::new(move || reads.load(std::sync::atomic::Ordering::SeqCst)),
        );
        assert!(al.allows_host("box.tail.ts.net"));
        exposure.set_identity(Some(crate::exposure::IdentityFacts {
            funnel_any: true,
            ..Default::default()
        }));
        assert!(
            !al.allows_host("box.tail.ts.net"),
            "a Funnel with no password withdraws the name"
        );
        assert!(al.allows_host("localhost"), "nothing else is refused");
        password.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            al.allows_host("box.tail.ts.net"),
            "with a password the Funnel's visitors reach the login page"
        );
    }

    /// The marker of a request that came through Funnel is a classification
    /// (the internet), never a refusal by this guard.
    #[tokio::test]
    async fn a_request_carrying_the_funnel_marker_passes_the_guard() {
        use tower::ServiceExt;
        let router = host_allowlist_layer(
            Router::new().route("/x", axum::routing::get(|| async { "ok" })),
            HostAllowlist::new(&ips(&["127.0.0.1"]), &[], true),
        );
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/x")
                    .header("Host", "localhost")
                    .header("Tailscale-Funnel-Request", "?1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn every_other_tailnet_name_is_still_refused() {
        let (al, _, _) = with_own_name("box.tail.ts.net");
        for host in [
            "other.tail.ts.net",
            "box.other.ts.net",
            "evil.box.tail.ts.net",
            "box.tail.ts.net.evil.com",
            "xbox.tail.ts.net",
            "tail.ts.net",
            "ts.net",
        ] {
            assert!(!al.allows_host(host), "{host} must be refused");
        }
    }

    #[test]
    fn the_own_name_is_refused_when_the_mode_is_no() {
        let (al, _, mode) = with_own_name("box.tail.ts.net");
        mode.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(
            !al.allows_host("box.tail.ts.net"),
            "a serve told to stay off the tailnet does not answer to its tailnet name"
        );
        mode.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            al.allows_host("box.tail.ts.net"),
            "and answers again on the way back"
        );
    }

    #[test]
    fn a_tailnet_rename_moves_the_allowed_name_without_rebuilding_the_guard() {
        let (al, names, _) = with_own_name("demo-box.old-tailnet.ts.net");
        assert!(al.allows_host("demo-box.old-tailnet.ts.net"));
        names.replace(&["demo-box.example-tailnet.ts.net".to_string()]);
        assert!(
            al.allows_host("demo-box.example-tailnet.ts.net"),
            "the new name"
        );
        assert!(
            !al.allows_host("demo-box.old-tailnet.ts.net"),
            "the old name belongs to nobody now, and is refused"
        );
        names.replace(&[]);
        assert!(
            !al.allows_host("demo-box.example-tailnet.ts.net"),
            "no name known, none allowed"
        );
    }

    #[test]
    fn a_guard_with_no_own_name_cell_allows_no_tailnet_name() {
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], true);
        assert!(!al.allows_host("box.tail.ts.net"));
    }

    // ── allowed_hosts, live ────────────────────────────────────────────────

    #[test]
    fn a_reloaded_allowed_hosts_list_applies_to_the_running_guard() {
        let configured = LiveHostNames::new(&["old.example.com:443".to_string()]);
        let al = HostAllowlist::new(&ips(&["127.0.0.1"]), &[], false)
            .with_live_configured_hosts(configured.clone());
        assert!(
            al.allows_host("old.example.com"),
            "port stripped on the way in"
        );
        configured.replace(&["New.Example.com".to_string()]);
        assert!(al.allows_host("new.example.com"), "the reloaded entry");
        assert!(!al.allows_host("old.example.com"), "the removed entry");
        configured.replace(&[]);
        assert!(!al.allows_host("new.example.com"));
        assert!(
            al.allows_host("localhost"),
            "loopback never depended on the list"
        );
    }

    /// A Host with a trailing dot (FQDN notation) is normalized before comparison.
    #[test]
    fn trailing_dot_normalized() {
        let al = HostAllowlist::new(&[], &["dux.example.com".to_string()], false);
        assert!(al.allows_host("dux.example.com."), "trailing dot stripped");
    }
}
