//! Tailscale address detection for LOCAL MODE serving.
//!
//! When `[server] tailscale_enabled` is on (the opt-out default), local mode
//! also binds the machine's Tailscale address so tailnet devices can reach dux
//! over WireGuard-encrypted transit. Detection shells out to the `tailscale ip`
//! CLI — the same tolerant pattern the `gh` integration uses: a missing CLI, a
//! down daemon, or garbage output degrades to `None` (with a reason for the
//! warning message), never an error that blocks loopback serving.

use std::net::IpAddr;

use crate::logger;

/// Why Tailscale address detection produced no usable address. Carried alongside
/// `None` so the caller can surface an accurate, actionable warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TailscaleUnavailable {
    /// The `tailscale` CLI is not installed or could not be executed.
    CommandMissing,
    /// The CLI ran but exited non-zero (daemon down, not logged in, etc.).
    CommandFailed,
    /// The CLI ran and succeeded but emitted no address we could parse.
    NoAddress,
}

impl TailscaleUnavailable {
    /// A short human reason for logs / status text.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::CommandMissing => "the tailscale CLI is not installed or not on PATH",
            Self::CommandFailed => {
                "the tailscale CLI failed (is the daemon running and logged in?)"
            }
            Self::NoAddress => "the tailscale CLI returned no usable address",
        }
    }
}

/// Detect this machine's Tailscale address by shelling out to `tailscale ip`.
///
/// Returns `Ok(addr)` with the preferred address, or `Err(reason)` when no
/// address is available. This NEVER blocks serving — the caller treats `Err` as
/// "serve loopback only" and warns. The CLI call follows the `gh`-availability
/// precedent: any failure to spawn maps to `CommandMissing`, a non-zero exit to
/// `CommandFailed`, and unparseable output to `NoAddress`.
pub fn detect_ip() -> Result<IpAddr, TailscaleUnavailable> {
    // `tailscale ip` (no args) prints one address per line: the IPv4 (100.64/10)
    // first, then the IPv6, when available.
    let output = match std::process::Command::new("tailscale").arg("ip").output() {
        Ok(output) => output,
        Err(err) => {
            logger::debug(&format!("[tailscale] could not run `tailscale ip`: {err}"));
            return Err(TailscaleUnavailable::CommandMissing);
        }
    };

    if !output.status.success() {
        logger::debug(&format!(
            "[tailscale] `tailscale ip` exited non-zero: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
        return Err(TailscaleUnavailable::CommandFailed);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_tailscale_ip(&text).ok_or(TailscaleUnavailable::NoAddress)
}

/// Pure parser for `tailscale ip` output. Prefers the first valid CGNAT IPv4
/// (100.64.0.0/10); when no such IPv4 is present, accepts the first IPv6 in
/// Tailscale's `fd7a:115c:a1e0::/48` ULA range. Returns `None` for empty or
/// unparseable output.
pub fn parse_tailscale_ip(output: &str) -> Option<IpAddr> {
    let mut ipv6_fallback: Option<IpAddr> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(ip) = trimmed.parse::<IpAddr>() else {
            continue;
        };
        match ip {
            IpAddr::V4(v4) if is_tailscale_cgnat(v4) => return Some(ip),
            IpAddr::V4(_) => {}
            IpAddr::V6(v6) if ipv6_fallback.is_none() && is_tailscale_ipv6(v6) => {
                ipv6_fallback = Some(ip);
            }
            IpAddr::V6(_) => {}
        }
    }

    ipv6_fallback
}

/// Whether `addr` is in Tailscale's CGNAT range 100.64.0.0/10 (RFC 6598).
fn is_tailscale_cgnat(addr: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = addr.octets();
    a == 100 && (64..=127).contains(&b)
}

/// Whether `addr` is in Tailscale's IPv6 ULA range `fd7a:115c:a1e0::/48`.
///
/// This is the EXACT block Tailscale assigns (the IPv6 mirror of the 100.64/10
/// CGNAT v4 leg), not "any global/ULA v6" — so a plain ULA (`fc00::1`), a
/// documentation address (`2001:db8::`), or a real global (`2606:…`) is rejected.
/// A /48 means the first three 16-bit segments must equal `fd7a:115c:a1e0`.
///
/// In practice this leg is effectively unreachable: the IPv4 CGNAT line is
/// preferred and is present on every normal tailnet, so [`parse_tailscale_ip`]
/// only consults this fallback on an IPv6-only tailnet.
fn is_tailscale_ipv6(addr: std::net::Ipv6Addr) -> bool {
    let [a, b, c, ..] = addr.segments();
    a == 0xfd7a && b == 0x115c && c == 0xa1e0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_first_cgnat_ipv4() {
        let out = "100.101.102.103\nfd7a:115c:a1e0::1234\n";
        assert_eq!(
            parse_tailscale_ip(out),
            Some("100.101.102.103".parse().unwrap())
        );
    }

    #[test]
    fn prefers_ipv4_even_when_ipv6_comes_first() {
        let out = "fd7a:115c:a1e0::1234\n100.64.0.1\n";
        assert_eq!(parse_tailscale_ip(out), Some("100.64.0.1".parse().unwrap()));
    }

    #[test]
    fn falls_back_to_tailscale_ipv6_when_no_cgnat_ipv4() {
        let out = "fd7a:115c:a1e0::1234\n";
        assert_eq!(
            parse_tailscale_ip(out),
            Some("fd7a:115c:a1e0::1234".parse().unwrap())
        );
    }

    #[test]
    fn accepts_first_and_last_tailscale_ipv6_in_range() {
        // The /48 boundary: the network address and the last address in
        // fd7a:115c:a1e0::/48 (the host portion is the low 80 bits) are both
        // accepted when no CGNAT v4 is present.
        assert_eq!(
            parse_tailscale_ip("fd7a:115c:a1e0::\n"),
            Some("fd7a:115c:a1e0::".parse().unwrap())
        );
        assert_eq!(
            parse_tailscale_ip("fd7a:115c:a1e0:ffff:ffff:ffff:ffff:ffff\n"),
            Some("fd7a:115c:a1e0:ffff:ffff:ffff:ffff:ffff".parse().unwrap())
        );
    }

    #[test]
    fn rejects_ipv6_outside_the_tailscale_48() {
        // One past the /48 (third segment a1e1), a plain ULA, a documentation
        // address, and a real global must all be rejected — the leg accepts ONLY
        // fd7a:115c:a1e0::/48, not "any global/ULA v6".
        assert_eq!(parse_tailscale_ip("fd7a:115c:a1e1::\n"), None);
        assert_eq!(parse_tailscale_ip("fc00::1\n"), None);
        assert_eq!(parse_tailscale_ip("2001:db8::1\n"), None);
        assert_eq!(parse_tailscale_ip("2606:4700:4700::1111\n"), None);
    }

    #[test]
    fn rejects_non_cgnat_ipv4() {
        // A plain LAN IPv4 is not a Tailscale CGNAT address, and a link-local
        // IPv6 is not a usable bind target — so nothing is returned.
        let out = "192.168.1.50\nfe80::1\n";
        assert_eq!(parse_tailscale_ip(out), None);
    }

    #[test]
    fn validates_cgnat_lower_and_upper_bounds() {
        // 100.63.x is BELOW the 100.64/10 range; 100.128.x is ABOVE it.
        assert_eq!(parse_tailscale_ip("100.63.255.255\n"), None);
        assert_eq!(parse_tailscale_ip("100.128.0.0\n"), None);
        // The exact boundaries are inside the range.
        assert_eq!(
            parse_tailscale_ip("100.64.0.0\n"),
            Some("100.64.0.0".parse().unwrap())
        );
        assert_eq!(
            parse_tailscale_ip("100.127.255.255\n"),
            Some("100.127.255.255".parse().unwrap())
        );
    }

    #[test]
    fn empty_output_yields_none() {
        assert_eq!(parse_tailscale_ip(""), None);
        assert_eq!(parse_tailscale_ip("\n  \n\t\n"), None);
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let out = "not an ip\n# comment\n100.64.5.6 extra tokens\n100.100.100.100\n";
        // "100.64.5.6 extra tokens" fails to parse (extra tokens), so the first
        // valid CGNAT address wins.
        assert_eq!(
            parse_tailscale_ip(out),
            Some("100.100.100.100".parse().unwrap())
        );
    }

    #[test]
    fn unavailable_reasons_are_descriptive() {
        assert!(
            TailscaleUnavailable::CommandMissing
                .reason()
                .contains("PATH")
        );
        assert!(
            TailscaleUnavailable::CommandFailed
                .reason()
                .contains("daemon")
        );
        assert!(
            TailscaleUnavailable::NoAddress
                .reason()
                .contains("no usable address")
        );
    }
}
