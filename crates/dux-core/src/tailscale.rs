//! Tailscale address detection for LOCAL MODE serving.
//!
//! Unless `[server] tailscale` is `"no"`, local mode also binds the machine's
//! Tailscale address so tailnet devices can reach dux over WireGuard-encrypted
//! transit. Detection shells out to the `tailscale ip` CLI, following the same
//! tolerant pattern the `gh` integration uses: a missing CLI, a down daemon, or
//! garbage output degrades to `None` (with a reason for the warning message), never an
//! error that blocks loopback serving.
//!
//! On `"auto"` this detection is not a one-shot at startup: the serve path polls
//! it for the whole run so the Tailscale listener can come and go with the
//! interface. That is why the call is BOUNDED (see [`detect_ip`]): a wedged
//! `tailscaled` is exactly the situation the watcher exists to survive, so it
//! must not be able to park the watcher forever.

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

/// The warning to show when the Tailscale address was wanted but not found at
/// startup. `serving` names what dux is serving instead ("loopback" for the flip,
/// "the configured host" for `dux server`), so both entry points read the same
/// sentence from one place.
///
/// The two modes end differently and the message has to say which one the reader
/// is in: on [`TailscaleMode::Auto`] this is a "not yet" and dux keeps looking,
/// on [`TailscaleMode::Yes`] it is settled for the whole run. Telling an `auto`
/// user their tailnet is unavailable, when dux is about to bind it the moment
/// tailscaled connects, is the kind of stale warning people learn to ignore.
///
/// [`TailscaleMode::No`] never reaches here (nothing is detected), and the
/// function stays exhaustive over the enum so a fourth mode is a compile error.
pub fn undetected_warning(
    mode: crate::config::TailscaleMode,
    reason: TailscaleUnavailable,
    serving: &str,
) -> String {
    use crate::config::TailscaleMode;
    match mode {
        TailscaleMode::Auto => format!(
            "Tailscale not detected ({}), so dux is serving on {serving} only for now. \
             It keeps watching and binds your Tailscale address by itself the moment the \
             interface appears, with no restart. Set tailscale = \"no\" in [server] to stop \
             looking and silence this.",
            reason.reason()
        ),
        TailscaleMode::Yes => format!(
            "Tailscale not detected ({}), so dux is serving on {serving} only for this whole \
             run: [server] tailscale = \"yes\" looks exactly once, at startup. Set \
             tailscale = \"auto\" to have dux bind it whenever the interface appears, or \
             \"no\" to silence this.",
            reason.reason()
        ),
        TailscaleMode::No => format!(
            "Tailscale not detected ({}), and [server] tailscale = \"no\" means dux was not \
             going to bind it anyway.",
            reason.reason()
        ),
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
    detect_ip_with("tailscale", DETECT_TIMEOUT)
}

/// Hard wall-clock cap on one `tailscale ip` call.
///
/// A few seconds, because this is a local query to a local daemon: it either
/// answers immediately or it is not going to. The cap matters because on the
/// `auto` mode this call is repeated for the whole life of the server, and a
/// `tailscaled` wedged by a suspend and resume must cost one skipped period
/// rather than a watcher that never checks again.
pub const DETECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// [`detect_ip`] with the program and the cap named, so the bounded behavior can
/// be exercised against a stand-in binary without touching the test process's
/// `PATH` (which is shared, and unsafe to mutate under a test runner). This is
/// the `gh` host-probe precedent.
///
/// A TIMEOUT maps to [`TailscaleUnavailable::CommandFailed`] rather than to a
/// variant of its own: from the caller's point of view a daemon that does not
/// answer and a daemon that answers with an error are the same situation, and
/// `CommandFailed`'s reason already asks the right question.
pub fn detect_ip_with(
    program: &str,
    timeout: std::time::Duration,
) -> Result<IpAddr, TailscaleUnavailable> {
    // `tailscale ip` (no args) prints one address per line: the IPv4 (100.64/10)
    // first, then the IPv6, when available.
    let mut cmd = std::process::Command::new(program);
    cmd.arg("ip");
    let output = match crate::bounded_command::run_command_with_timeout(
        cmd,
        timeout,
        crate::bounded_command::DEFAULT_READER_DRAIN,
        "tailscale",
    ) {
        crate::bounded_command::CommandOutcome::Completed(output) => output,
        crate::bounded_command::CommandOutcome::TimedOut => {
            logger::debug(&format!(
                "[tailscale] `{program} ip` did not answer within {:?} and was killed",
                timeout
            ));
            return Err(TailscaleUnavailable::CommandFailed);
        }
        crate::bounded_command::CommandOutcome::Failed(err) => {
            logger::debug(&format!("[tailscale] could not run `{program} ip`: {err}"));
            return Err(TailscaleUnavailable::CommandMissing);
        }
    };

    if !output.status.success() {
        logger::debug(&format!(
            "[tailscale] `{program} ip` exited non-zero: {}",
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
pub fn is_tailscale_cgnat(addr: std::net::Ipv4Addr) -> bool {
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
pub fn is_tailscale_ipv6(addr: std::net::Ipv6Addr) -> bool {
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

    /// A throwaway executable stand-in for the `tailscale` CLI, named by absolute
    /// path so nothing has to mutate the test process's shared `PATH`. This is the
    /// `gh` host-probe precedent, with the body written per test: a stand-in that
    /// ignores its `ip` argument is the only way to prove anything about a wedged
    /// CLI, because a real program handed an argument it cannot parse just exits
    /// non-zero immediately.
    struct StandIn {
        path: std::path::PathBuf,
    }

    impl StandIn {
        fn new(name: &str, body: &str) -> Self {
            use std::io::Write;
            use std::os::unix::fs::PermissionsExt;
            let path = std::env::temp_dir().join(format!(
                "dux-tailscale-stand-in-{}-{name}",
                std::process::id()
            ));
            let mut file = std::fs::File::create(&path).expect("create the stand-in");
            writeln!(file, "#!/bin/sh\n{body}").expect("write the stand-in");
            drop(file);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make the stand-in executable");
            Self { path }
        }

        fn program(&self) -> &str {
            self.path.to_str().expect("a UTF-8 temp path")
        }
    }

    impl Drop for StandIn {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn a_wedged_tailscale_cli_times_out_and_reports_a_failure() {
        // The whole reason the call is bounded. On "auto" this runs for the life
        // of the server, so a tailscaled that stopped answering (a suspend and
        // resume, which is exactly the case the watcher serves) must cost one
        // timeout and not the watcher itself.
        //
        // The stand-in ignores its `ip` argument and sleeps far past the cap, so
        // the ONLY way out of the call is the timeout: with the cap removed this
        // test parks for thirty seconds instead of passing. It `exec`s the sleep
        // so the process the runner kills is the sleep itself, leaving no orphan
        // behind. Asserting the elapsed time is at least the cap is what proves
        // the timeout, and not some other early exit, is what ended the call.
        let cli = StandIn::new("wedged", "exec sleep 30");
        let cap = std::time::Duration::from_millis(300);
        let start = std::time::Instant::now();
        let result = detect_ip_with(cli.program(), cap);
        let elapsed = start.elapsed();
        assert_eq!(result, Err(TailscaleUnavailable::CommandFailed));
        assert!(
            elapsed >= cap,
            "the call must have run into the cap, not exited early: took {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "a wedged CLI must not park the caller for its whole sleep, took {elapsed:?}"
        );
    }

    #[test]
    fn a_missing_tailscale_cli_is_reported_as_missing_not_as_a_failure() {
        assert_eq!(
            detect_ip_with("dux-no-such-tailscale-9f3a", DETECT_TIMEOUT),
            Err(TailscaleUnavailable::CommandMissing),
            "the operator needs to know it is not installed, not that it failed"
        );
    }

    #[test]
    fn a_stand_in_cli_that_prints_an_address_is_parsed() {
        // Proves the bounded path really reads stdout, and not just that the
        // program exited zero. The stand-in ignores its `ip` argument and prints
        // one CGNAT address, which is what `tailscale ip` does on a normal
        // tailnet.
        let cli = StandIn::new("address", "echo 100.64.0.7");
        assert_eq!(
            detect_ip_with(cli.program(), DETECT_TIMEOUT),
            Ok("100.64.0.7".parse().unwrap()),
            "the address the CLI printed must reach the caller"
        );
    }

    #[test]
    fn a_stand_in_cli_that_succeeds_with_no_output_has_no_address() {
        // The other half: exiting zero is not an answer. `true` ignores the `ip`
        // argument and prints nothing, so there is nothing to parse.
        assert_eq!(
            detect_ip_with("true", DETECT_TIMEOUT),
            Err(TailscaleUnavailable::NoAddress),
            "a CLI that succeeds with no output has no address to give"
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
