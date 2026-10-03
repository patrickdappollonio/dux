//! Tailscale address detection for LOCAL MODE serving.
//!
//! Unless `[server] tailscale` is `"no"`, local mode also binds the machine's
//! Tailscale address so tailnet devices can reach dux. Detection shells out to
//! the `tailscale ip` CLI and is tolerant: a missing CLI, a down daemon or
//! garbage output degrades to `None` with a reason for the warning, never an
//! error that blocks loopback serving.
//!
//! On `"auto"` the serve path polls this for the whole run, so the listener can
//! come and go with the interface. That is why the call is BOUNDED (see
//! [`detect_ip`]): a wedged `tailscaled` is the situation the watcher exists to
//! survive, so it must not park the watcher forever.
//!
//! The same watcher also asks the CLI how this machine is NAMED: its own
//! MagicDNS name (`tailscale status --json`), which the Host guard admits, and
//! the `tailscale serve` routes that end at dux's port (`tailscale serve status
//! --json`), whose HTTPS URL dux shows. Both reads are bounded the same way, and
//! dux only ever reads the serve configuration: it never runs `tailscale serve`
//! itself, because that is a persistent change to the machine that may need an
//! administrator and fails outright where HTTPS certificates are switched off.

use std::net::IpAddr;

use crate::logger;

/// The keyed-status key a mode change reports under when no operation is waiting
/// on it.
///
/// A mode change a surface asked for resolves its own op on its own key; this is
/// for the answer that arrives after the asking surface has forgotten it (a stop
/// and start across the request), which is still a listener changing and still
/// worth saying.
pub const MODE_CHANGE_STATUS_KEY: &str = "tailscale-mode";

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
    /// The CLI ran and said it could not reach the Tailscale daemon at all (any
    /// dial error: no daemon, a permission, a wrong socket).
    DaemonUnreachable,
    /// The CLI is missing or cannot reach its daemon, yet this machine HAS a
    /// Tailscale address, so a daemon is running that nothing here can ask
    /// about its Funnels. Unknown, never "no Funnel".
    Unverifiable,
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
            Self::DaemonUnreachable => "the tailscale CLI could not reach the Tailscale daemon",
            Self::Unverifiable => {
                "this machine has a Tailscale address, but the tailscale CLI is missing or \
                 cannot reach the daemon"
            }
        }
    }
}

/// The warning to show when the Tailscale address was wanted but not found at
/// startup. `serving` names what dux is serving instead ("loopback" for the flip,
/// "the configured host" for `dux server`), so both entry points read the same
/// sentence from one place.
///
/// The message has to say which mode the reader is in: on
/// [`TailscaleMode::Auto`] this is a "not yet" and dux keeps looking, while on
/// [`TailscaleMode::Yes`] nothing looks again until the mode changes.
/// [`TailscaleMode::No`] never reaches here, and the function stays exhaustive
/// over the enum so a fourth mode is a compile error.
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
            "Tailscale not detected ({}), so dux is serving on {serving} only: [server] \
             tailscale = \"yes\" looks exactly once and does not look again by itself. Set \
             tailscale = \"auto\" to have dux bind it whenever the interface appears, or \
             \"no\" to silence this. Either way you can change the mode while dux runs, \
             from the TUI palette or the web Preferences dialog.",
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
/// address is available. This NEVER blocks serving: the caller treats `Err` as
/// "serve loopback only" and warns. The CLI call follows the `gh`-availability
/// precedent: any failure to spawn maps to `CommandMissing`, a non-zero exit to
/// `CommandFailed`, and unparseable output to `NoAddress`.
pub fn detect_ip() -> Result<IpAddr, TailscaleUnavailable> {
    first_cli_answer(CLI_CANDIDATES, |program| {
        detect_ip_with(program, DETECT_TIMEOUT)
    })
}

/// Where the `tailscale` CLI is looked for, in order: `PATH` first, then where
/// Tailscale's macOS clients put it when `PATH` does not have it (Tailscale's
/// CLI docs): the Standalone client's launcher and the App Store client's
/// bundled CLI.
pub const CLI_CANDIDATES: &[&str] = &[
    "tailscale",
    "/usr/local/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

/// Run `ask` with each candidate in turn, moving on only while the CLI is not
/// there at all.
fn first_cli_answer<T>(
    programs: &[&str],
    ask: impl Fn(&str) -> Result<T, TailscaleUnavailable>,
) -> Result<T, TailscaleUnavailable> {
    let mut last = Err(TailscaleUnavailable::CommandMissing);
    for program in programs {
        last = ask(program);
        if !matches!(last, Err(TailscaleUnavailable::CommandMissing)) {
            return last;
        }
    }
    last
}

/// Every address on this machine's network interfaces, read with
/// `getifaddrs`. Empty when they cannot be read.
pub fn interface_addresses() -> Vec<IpAddr> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` fills `list` with a linked list it allocated, which
    // is walked read-only below and handed back to `freeifaddrs` exactly once.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Vec::new();
    }
    let mut addrs = Vec::new();
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: `cursor` is a node of the list `getifaddrs` returned and has
        // not been freed; `ifa_addr` is null or points at a sockaddr whose
        // family field says which concrete type it is.
        unsafe {
            let entry = &*cursor;
            let sockaddr = entry.ifa_addr;
            if !sockaddr.is_null() {
                match i32::from((*sockaddr).sa_family) {
                    libc::AF_INET => {
                        let v4 = &*(sockaddr as *const libc::sockaddr_in);
                        addrs.push(IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(
                            v4.sin_addr.s_addr,
                        ))));
                    }
                    libc::AF_INET6 => {
                        let v6 = &*(sockaddr as *const libc::sockaddr_in6);
                        addrs.push(IpAddr::V6(std::net::Ipv6Addr::from(v6.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            cursor = entry.ifa_next;
        }
    }
    // SAFETY: `list` came from a successful `getifaddrs` and is freed once.
    unsafe { libc::freeifaddrs(list) };
    addrs
}

/// Whether any of `addrs` is a Tailscale address (100.64.0.0/10 or
/// fd7a:115c:a1e0::/48).
pub fn has_tailscale_address(addrs: &[IpAddr]) -> bool {
    addrs.iter().any(|ip| match ip {
        IpAddr::V4(v4) => is_tailscale_cgnat(*v4),
        IpAddr::V6(v6) => is_tailscale_ipv6(*v6),
    })
}

/// Whether this machine has a Tailscale address on any interface right now.
pub fn machine_has_tailscale_address() -> bool {
    has_tailscale_address(&interface_addresses())
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
/// `PATH`, which is shared and unsafe to mutate under a test runner.
///
/// A TIMEOUT maps to [`TailscaleUnavailable::CommandFailed`] rather than to a
/// variant of its own: to the caller, a daemon that does not answer and one that
/// answers with an error are the same situation.
pub fn detect_ip_with(
    program: &str,
    timeout: std::time::Duration,
) -> Result<IpAddr, TailscaleUnavailable> {
    // `tailscale ip` (no args) prints one address per line: the IPv4 (100.64/10)
    // first, then the IPv6, when available.
    let text = run_cli(program, &["ip"], timeout)?;
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

/// What `tailscale status --json` says about THIS machine's name on the tailnet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelfStatus {
    /// This machine's MagicDNS name (`Self.DNSName`), lowercased with its
    /// trailing dot removed. `None` when the daemon is not running, the field is
    /// missing or empty, or it is not a plain DNS name.
    pub dns_name: Option<String>,
    /// Whether MagicDNS is switched on for the tailnet. Read from
    /// `CurrentTailnet.MagicDNSEnabled`; a CLI too old to report it counts as on
    /// when it reports a MagicDNS suffix at all.
    pub magic_dns_enabled: bool,
    /// The tailnet's MagicDNS suffix (`example.ts.net`), when reported.
    pub magic_dns_suffix: Option<String>,
    /// The names Tailscale can issue an HTTPS certificate for. Empty when HTTPS
    /// certificates are switched off for the tailnet.
    pub cert_domains: Vec<String>,
}

/// One `tailscale serve` route that ends at dux.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServeRoute {
    /// The URL a tailnet device opens, such as
    /// `https://box.example.ts.net:8443`. A default port is left out.
    pub url: String,
    /// Whether Tailscale Funnel publishes this route to the public internet.
    pub funnel: bool,
}

/// Everything one look at the Tailscale CLI says about how this machine is
/// named and published on the tailnet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TailscaleIdentity {
    pub status: SelfStatus,
    /// The `tailscale serve` routes that end at dux's port.
    pub serve: Vec<ServeRoute>,
    /// Whether Tailscale Funnel is switched on for ANY route on this machine,
    /// whatever it forwards to. See [`parse_serve_funnel`].
    pub funnel: bool,
    /// Whether a Funnel publishes something that forwards to dux's port: a raw
    /// TCP forward or a web handler. See [`parse_funnel_to_port`].
    pub funnel_to_dux: bool,
}

/// Ask the `tailscale` CLI for this machine's name and the serve routes that
/// end at `dux_port`. Bounded exactly like [`detect_ip`]; see
/// [`detect_identity_with`].
pub fn detect_identity(dux_port: u16) -> Result<TailscaleIdentity, TailscaleUnavailable> {
    detect_identity_checked(
        CLI_CANDIDATES,
        DETECT_TIMEOUT,
        dux_port,
        &machine_has_tailscale_address,
    )
}

/// [`detect_identity`] over a list of candidate programs, with the question
/// "does this machine have a Tailscale address" injected.
///
/// A CLI that is missing everywhere, or that cannot reach a daemon, only means
/// "no Funnel is possible" when this machine has NO Tailscale address: then no
/// daemon is up, and nothing can publish anything. With an address present, a
/// daemon IS running that dux cannot ask, so the answer is
/// [`TailscaleUnavailable::Unverifiable`].
pub fn detect_identity_checked(
    programs: &[&str],
    timeout: std::time::Duration,
    dux_port: u16,
    has_tailscale_address: &dyn Fn() -> bool,
) -> Result<TailscaleIdentity, TailscaleUnavailable> {
    match first_cli_answer(programs, |program| {
        detect_identity_with(program, timeout, dux_port)
    }) {
        Err(TailscaleUnavailable::CommandMissing | TailscaleUnavailable::DaemonUnreachable)
            if has_tailscale_address() =>
        {
            Err(TailscaleUnavailable::Unverifiable)
        }
        other => other,
    }
}

/// [`detect_identity`] with the program and the cap named, for the stand-in
/// tests. Runs `status --json --peers=false` (the peers are not needed, and on a
/// large tailnet they are most of the document) and then `serve status --json`,
/// each under its own `timeout`.
///
/// Either command failing makes the whole answer a failure rather than half of
/// one: the caller keeps what it already knew, because an unknown answer is not
/// a change, and a serve route that flickered away for one look would otherwise
/// say "gone" and "back" to every surface.
pub fn detect_identity_with(
    program: &str,
    timeout: std::time::Duration,
    dux_port: u16,
) -> Result<TailscaleIdentity, TailscaleUnavailable> {
    // `--peers=false` keeps the document small on a big tailnet; a CLI too old
    // to know the flag is asked again without it.
    let status = match run_cli_raw(program, &["status", "--json", "--peers=false"], timeout) {
        Err(failure) if failure.stderr.contains("flag provided but not defined") => {
            run_cli(program, &["status", "--json"], timeout)?
        }
        other => other.map_err(|failure| failure.kind)?,
    };
    let status = parse_status_json(&status).ok_or(TailscaleUnavailable::NoAddress)?;
    // A CLI built without `serve` (`ts_omit_serve`) has a daemon that cannot
    // serve, so it has no Funnel to report; any other failure stays one.
    let serve = match run_cli_raw(program, &["serve", "status", "--json"], timeout) {
        Ok(serve) => serve,
        Err(failure) if failure.stderr.contains("unknown subcommand") => "{}".to_string(),
        Err(failure) => return Err(failure.kind),
    };
    let funnel = parse_serve_funnel(&serve).ok_or(TailscaleUnavailable::NoAddress)?;
    let funnel_to_dux =
        parse_funnel_to_port(&serve, dux_port).ok_or(TailscaleUnavailable::NoAddress)?;
    let serve = parse_serve_status_json(&serve, dux_port).ok_or(TailscaleUnavailable::NoAddress)?;
    Ok(TailscaleIdentity {
        status,
        serve,
        funnel,
        funnel_to_dux,
    })
}

/// Run one bounded `tailscale` CLI call and hand back its stdout. A spawn
/// failure is a missing CLI, a timeout or a non-zero exit is a failure.
fn run_cli(
    program: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<String, TailscaleUnavailable> {
    run_cli_raw(program, args, timeout).map_err(|failure| failure.kind)
}

/// Why one CLI call failed, with what it printed, for the callers that read it.
struct CliFailure {
    kind: TailscaleUnavailable,
    stderr: String,
}

impl From<TailscaleUnavailable> for CliFailure {
    fn from(kind: TailscaleUnavailable) -> Self {
        Self {
            kind,
            stderr: String::new(),
        }
    }
}

fn run_cli_raw(
    program: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<String, CliFailure> {
    let shown = args.join(" ");
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    let output = match crate::bounded_command::run_command_with_timeout(
        cmd,
        timeout,
        crate::bounded_command::DEFAULT_READER_DRAIN,
        "tailscale",
    ) {
        crate::bounded_command::CommandOutcome::Completed(output) => output,
        crate::bounded_command::CommandOutcome::TimedOut => {
            logger::debug(&format!(
                "[tailscale] `{program} {shown}` did not answer within {timeout:?} and was killed"
            ));
            return Err(TailscaleUnavailable::CommandFailed.into());
        }
        crate::bounded_command::CommandOutcome::Failed(err) => {
            logger::debug(&format!(
                "[tailscale] could not run `{program} {shown}`: {err}"
            ));
            return Err(TailscaleUnavailable::CommandMissing.into());
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        logger::debug(&format!(
            "[tailscale] `{program} {shown}` exited non-zero: {}",
            stderr.trim(),
        ));
        // The CLI's own words for a daemon whose socket cannot be dialled
        // (client/local/local.go): "Failed to connect to local Tailscale daemon".
        let kind = if stderr
            .to_ascii_lowercase()
            .contains("failed to connect to local tailscale daemon")
        {
            TailscaleUnavailable::DaemonUnreachable
        } else {
            TailscaleUnavailable::CommandFailed
        };
        return Err(CliFailure {
            kind,
            stderr: stderr.into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether the serve configuration switches Funnel on for anything at all: any
/// `AllowFunnel` entry that is `true`, at any depth (the top level, a
/// foreground session, a service), whatever port it names and whatever that
/// port forwards to: a web handler, a path, a TCP forward, a TLS-terminated
/// forward. `None` when the text is not a JSON object.
///
/// Deliberately broader than [`parse_serve_status_json`]'s idea of a route to
/// dux. That one decides what to SHOW; this one decides whether the Host guard
/// may answer to this machine's name, and a Funnel route that reaches dux by a
/// way the display filter does not recognise (the Tailscale address, a LAN
/// address, a second proxy) must still count.
pub fn parse_serve_funnel(text: &str) -> Option<bool> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.as_object()?;
    Some(any_funnel(&value))
}

/// Whether a Funnel publishes anything that forwards to `dux_port`, on a port
/// some `AllowFunnel` entry of the same config switches on: a `TCP` entry with a
/// `TCPForward` (TLS-terminated or not), or a `Web` handler (any path) whose
/// `Proxy` names dux's port. Checked at the top level and in every foreground
/// session. `None` when the text is not a JSON object.
///
/// Any target host counts, not only loopback: a forward to this machine's
/// Tailscale address, a LAN address or the wildcard reaches dux just the same,
/// and telling this machine's addresses from another's is not something a
/// guard should guess at. A forward to the same port on ANOTHER machine is
/// treated the same way, which only ever refuses more.
///
/// Neither shape can be told apart by the Host guard: a raw TCP stream carries
/// no `Tailscale-Funnel-Request` header, a web request through a daemon older
/// than 1.72 does not either, both arrive from loopback, and both can claim any
/// Host they like. So dux refuses everything while either stands.
pub fn parse_funnel_to_port(text: &str, dux_port: u16) -> Option<bool> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.as_object()?;
    Some(any_funnel(&value) && any_target_to_port(&value, dux_port))
}

/// Whether any `TCPForward` or `Proxy` anywhere in the document names
/// `dux_port`.
fn any_target_to_port(value: &serde_json::Value, dux_port: u16) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, inner)| {
            let names_dux = (key == "TCPForward" || key == "Proxy")
                && inner
                    .as_str()
                    .is_some_and(|target| target_port(target) == Some(dux_port));
            names_dux || any_target_to_port(inner, dux_port)
        }),
        serde_json::Value::Array(items) => {
            items.iter().any(|item| any_target_to_port(item, dux_port))
        }
        _ => false,
    }
}

/// The port a forward or proxy target names: `host:port`, `scheme://host:port/path`,
/// a bare port, or, with no port at all, the scheme's default (`http` 80,
/// `https` 443).
fn target_port(target: &str) -> Option<u16> {
    let (scheme, rest) = match target.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, target),
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let authority = authority
        .rsplit_once(']')
        .map_or(authority, |(_, after)| after);
    match authority.rsplit_once(':') {
        Some((_, port)) => port.parse().ok(),
        None => authority.parse().ok().or(match scheme {
            Some(s) if s.eq_ignore_ascii_case("http") => Some(80),
            Some(s) if s.to_ascii_lowercase().starts_with("https") => Some(443),
            _ => None,
        }),
    }
}

fn any_funnel(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, inner)| {
            let allowed_here = key == "AllowFunnel"
                && inner
                    .as_object()
                    .is_some_and(|entries| entries.values().any(|v| v.as_bool() == Some(true)));
            allowed_here || any_funnel(inner)
        }),
        serde_json::Value::Array(items) => items.iter().any(any_funnel),
        _ => false,
    }
}

impl SelfStatus {
    /// This machine's name, but only when Tailscale itself assigned it: it ends
    /// with the tailnet's MagicDNS suffix, and that suffix is under `ts.net`.
    /// A name from another control server (Headscale, say) sits in a domain
    /// its operator chose, so it is not one dux admits by itself.
    pub fn tailscale_assigned_name(&self) -> Option<&str> {
        let name = self.dns_name.as_deref()?;
        let suffix = self.magic_dns_suffix.as_deref()?;
        let under_ts_net = suffix.ends_with(".ts.net");
        let in_suffix = name
            .strip_suffix(suffix)
            .is_some_and(|host| host.len() > 1 && host.ends_with('.'));
        (under_ts_net && in_suffix).then_some(name)
    }
}

/// Parse `tailscale status --json`. `None` when the text is not a JSON object.
///
/// Every field is optional and a field of the wrong type reads as absent, so a
/// CLI that grows or renames a field degrades to "no name" rather than to an
/// error: the name only ever ADDS a Host the guard accepts, so the safe reading
/// of an answer dux does not understand is the one that adds nothing.
pub fn parse_status_json(text: &str) -> Option<SelfStatus> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let root = value.as_object()?;
    let running = root.get("BackendState").and_then(|v| v.as_str()) == Some("Running");
    let dns_name = if running {
        root.get("Self")
            .and_then(|s| s.get("DNSName"))
            .and_then(|n| n.as_str())
            .and_then(normalize_dns_name)
    } else {
        None
    };
    let magic_dns_suffix = root
        .get("MagicDNSSuffix")
        .and_then(|v| v.as_str())
        .and_then(normalize_dns_name);
    let magic_dns_enabled = root
        .get("CurrentTailnet")
        .and_then(|t| t.get("MagicDNSEnabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(magic_dns_suffix.is_some());
    let cert_domains = root
        .get("CertDomains")
        .and_then(|v| v.as_array())
        .map(|domains| {
            domains
                .iter()
                .filter_map(|d| d.as_str())
                .filter_map(normalize_dns_name)
                .collect()
        })
        .unwrap_or_default();
    Some(SelfStatus {
        dns_name,
        magic_dns_enabled,
        magic_dns_suffix,
        cert_domains,
    })
}

/// Lowercase a DNS name and drop one trailing dot, or `None` when what is left
/// is not a plain hostname: dot-separated labels of ASCII letters, digits and
/// hyphens, none of them empty.
pub fn normalize_dns_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let name = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if name.is_empty() {
        return None;
    }
    let valid = name.split('.').all(|label| {
        !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    valid.then(|| name.to_ascii_lowercase())
}

/// Parse `tailscale serve status --json` and keep only the routes that end at
/// dux. `None` when the text is not a JSON object.
///
/// A route counts when its ROOT handler (`/`) proxies to `dux_port` on this
/// machine's loopback over plain HTTP. A handler mounted under a path is not
/// counted, because dux's pages load their assets from the root, and an
/// `https://` target cannot be dux, which speaks plain HTTP. The routes kept
/// under a foreground `tailscale serve` session (one run without `--bg`) count
/// as well as the background ones.
pub fn parse_serve_status_json(text: &str, dux_port: u16) -> Option<Vec<ServeRoute>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let root = value.as_object()?;
    let mut routes = Vec::new();
    collect_serve_routes(root, dux_port, &mut routes);
    if let Some(sessions) = root.get("Foreground").and_then(|f| f.as_object()) {
        for session in sessions.values().filter_map(|s| s.as_object()) {
            collect_serve_routes(session, dux_port, &mut routes);
        }
    }
    routes.sort();
    routes.dedup();
    Some(routes)
}

/// The dux routes in ONE serve config (the top level, or one foreground session).
fn collect_serve_routes(
    config: &serde_json::Map<String, serde_json::Value>,
    dux_port: u16,
    out: &mut Vec<ServeRoute>,
) {
    let Some(web) = config.get("Web").and_then(|w| w.as_object()) else {
        return;
    };
    for (host_port, site) in web {
        let ends_at_dux = site
            .get("Handlers")
            .and_then(|h| h.get("/"))
            .and_then(|h| h.get("Proxy"))
            .and_then(|p| p.as_str())
            .is_some_and(|proxy| proxy_reaches_loopback_port(proxy, dux_port));
        if !ends_at_dux {
            continue;
        }
        let Some((host, port)) = host_port.rsplit_once(':') else {
            continue;
        };
        let (Some(host), Ok(port)) = (normalize_dns_name(host), port.parse::<u16>()) else {
            continue;
        };
        let listener = config.get("TCP").and_then(|tcp| tcp.get(port.to_string()));
        let plain_http = listener
            .and_then(|l| l.get("HTTP"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && !listener
                .and_then(|l| l.get("HTTPS"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        let (scheme, default_port) = if plain_http {
            ("http", 80)
        } else {
            ("https", 443)
        };
        let url = if port == default_port {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}:{port}")
        };
        let funnel = config
            .get("AllowFunnel")
            .and_then(|f| f.get(host_port.as_str()))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        out.push(ServeRoute { url, funnel });
    }
}

/// Whether a serve handler's `Proxy` target is `port` on this machine's
/// loopback over plain HTTP, written with or without the `http://` scheme.
fn proxy_reaches_loopback_port(proxy: &str, port: u16) -> bool {
    let rest = match proxy.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some(_) => return false,
        None => proxy,
    };
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    if !(path.is_empty() || path == "/") {
        return false;
    }
    let Some((host, target_port)) = authority.rsplit_once(':') else {
        return false;
    };
    if target_port.parse::<u16>() != Ok(port) {
        return false;
    }
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether `addr` is in Tailscale's CGNAT range 100.64.0.0/10 (RFC 6598).
pub fn is_tailscale_cgnat(addr: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = addr.octets();
    a == 100 && (64..=127).contains(&b)
}

/// Whether `addr` is in Tailscale's IPv6 ULA range `fd7a:115c:a1e0::/48`.
///
/// The EXACT block Tailscale assigns (the IPv6 mirror of the 100.64/10 CGNAT v4
/// leg), not "any global or ULA v6", so a plain ULA (`fc00::1`), a documentation
/// address (`2001:db8::`) or a real global is rejected. A /48 means the first
/// three 16-bit segments must equal `fd7a:115c:a1e0`.
///
/// [`parse_tailscale_ip`] only consults this fallback on an IPv6-only tailnet,
/// since the IPv4 CGNAT line is preferred and present on every normal tailnet.
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
        // address, and a real global must all be rejected: the leg accepts ONLY
        // fd7a:115c:a1e0::/48, not "any global/ULA v6".
        assert_eq!(parse_tailscale_ip("fd7a:115c:a1e1::\n"), None);
        assert_eq!(parse_tailscale_ip("fc00::1\n"), None);
        assert_eq!(parse_tailscale_ip("2001:db8::1\n"), None);
        assert_eq!(parse_tailscale_ip("2606:4700:4700::1111\n"), None);
    }

    #[test]
    fn rejects_non_cgnat_ipv4() {
        // A plain LAN IPv4 is not a Tailscale CGNAT address, and a link-local
        // IPv6 is not a usable bind target, so nothing is returned.
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
    pub(super) struct StandIn {
        path: std::path::PathBuf,
    }

    impl StandIn {
        pub(super) fn new(name: &str, body: &str) -> Self {
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
            // Another test thread forking a child while the write handle above
            // was open copies that handle into its child until its exec closes
            // it, and executing the script inside that window fails with
            // "text file busy", which the probe reports as a missing command.
            // Wait until the script actually runs before handing it out.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match std::process::Command::new(&path).arg("ip").output() {
                    Ok(_) => break,
                    Err(_) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(err) => panic!("the stand-in never became runnable: {err}"),
                }
            }
            Self { path }
        }

        pub(super) fn program(&self) -> &str {
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

#[cfg(test)]
mod identity_tests {
    use super::*;

    const STATUS: &str = include_str!("../tests/fixtures/tailscale_status_self.json");
    const SERVE: &str = include_str!("../tests/fixtures/tailscale_serve_status.json");

    #[test]
    fn the_real_status_shape_names_this_machine_in_lowercase_without_the_trailing_dot() {
        let status = parse_status_json(STATUS).expect("a real status document parses");
        assert_eq!(
            status.dns_name.as_deref(),
            Some("demo-box.example-tailnet.ts.net")
        );
        assert!(status.magic_dns_enabled);
        assert_eq!(
            status.magic_dns_suffix.as_deref(),
            Some("example-tailnet.ts.net")
        );
        assert_eq!(status.cert_domains, vec!["demo-box.example-tailnet.ts.net"]);
    }

    #[test]
    fn a_status_that_is_not_json_or_not_an_object_is_no_answer() {
        assert_eq!(parse_status_json(""), None);
        assert_eq!(parse_status_json("not json"), None);
        assert_eq!(parse_status_json("[1, 2]"), None);
        assert_eq!(parse_status_json("{\"Self\": "), None);
    }

    #[test]
    fn a_status_with_no_usable_name_parses_with_none() {
        let status = parse_status_json("{\"BackendState\": \"Running\"}").unwrap();
        assert_eq!(status.dns_name, None, "no Self block at all");
        for name in [
            "",
            ".",
            "has space.ts.net.",
            "under_score.ts.net",
            "a..b.ts.net",
        ] {
            let text =
                format!("{{\"BackendState\": \"Running\", \"Self\": {{\"DNSName\": \"{name}\"}}}}");
            assert_eq!(
                parse_status_json(&text).unwrap().dns_name,
                None,
                "{name:?} is not a name dux may allow"
            );
        }
        let status =
            parse_status_json("{\"BackendState\": \"Running\", \"Self\": {\"DNSName\": 7}}")
                .unwrap();
        assert_eq!(status.dns_name, None, "a name of the wrong JSON type");
    }

    #[test]
    fn a_daemon_that_is_not_running_has_no_name_to_allow() {
        // A logged-out or stopped daemon still remembers the old name, and that
        // name is not this machine's while it is off the tailnet.
        for state in ["Stopped", "NeedsLogin", "Starting", "NoState"] {
            let text = STATUS.replace("\"Running\"", &format!("\"{state}\""));
            assert_eq!(parse_status_json(&text).unwrap().dns_name, None, "{state}");
        }
    }

    #[test]
    fn a_name_without_a_trailing_dot_and_a_name_with_one_read_the_same() {
        let with = parse_status_json(
            "{\"BackendState\": \"Running\", \"Self\": {\"DNSName\": \"Box.Tail.ts.net.\"}}",
        )
        .unwrap();
        let without = parse_status_json(
            "{\"BackendState\": \"Running\", \"Self\": {\"DNSName\": \"box.tail.ts.net\"}}",
        )
        .unwrap();
        assert_eq!(with.dns_name.as_deref(), Some("box.tail.ts.net"));
        assert_eq!(with.dns_name, without.dns_name);
    }

    #[test]
    fn magic_dns_off_tailnet_wide_is_reported_and_a_missing_flag_falls_back_to_the_suffix() {
        let off = STATUS.replace("\"MagicDNSEnabled\": true", "\"MagicDNSEnabled\": false");
        assert!(!parse_status_json(&off).unwrap().magic_dns_enabled);
        // An older CLI with no CurrentTailnet block: a suffix means MagicDNS.
        let old = "{\"BackendState\": \"Running\", \"MagicDNSSuffix\": \"tail.ts.net\", \
                   \"Self\": {\"DNSName\": \"box.tail.ts.net.\"}}";
        assert!(parse_status_json(old).unwrap().magic_dns_enabled);
        let none = "{\"BackendState\": \"Running\", \"Self\": {\"DNSName\": \"box.tail.ts.net.\"}}";
        assert!(!parse_status_json(none).unwrap().magic_dns_enabled);
    }

    #[test]
    fn https_certificates_switched_off_reads_as_no_cert_domains() {
        let off = STATUS.replace(
            "\"CertDomains\": [\n    \"demo-box.example-tailnet.ts.net\"\n  ]",
            "\"CertDomains\": null",
        );
        assert_ne!(
            off, STATUS,
            "precondition: the fixture's CertDomains was replaced"
        );
        assert!(parse_status_json(&off).unwrap().cert_domains.is_empty());
    }

    // ── tailscale serve status --json ──────────────────────────────────────

    fn route(url: &str) -> ServeRoute {
        ServeRoute {
            url: url.to_string(),
            funnel: false,
        }
    }

    #[test]
    fn the_real_serve_shape_pointing_at_dux_gives_its_https_url() {
        assert_eq!(
            parse_serve_status_json(SERVE, 3890),
            Some(vec![route("https://demo-box.example-tailnet.ts.net:8443")])
        );
    }

    #[test]
    fn a_serve_pointing_at_another_port_is_not_dux() {
        assert_eq!(parse_serve_status_json(SERVE, 3891), Some(vec![]));
    }

    #[test]
    fn no_serve_config_at_all_is_an_empty_answer_not_no_answer() {
        // What `tailscale serve status --json` prints when nothing is served.
        assert_eq!(parse_serve_status_json("{}", 3890), Some(vec![]));
        assert_eq!(parse_serve_status_json("{}\n", 3890), Some(vec![]));
    }

    #[test]
    fn malformed_serve_status_is_no_answer() {
        assert_eq!(parse_serve_status_json("", 3890), None);
        assert_eq!(parse_serve_status_json("No serve config", 3890), None);
        assert_eq!(parse_serve_status_json("[]", 3890), None);
    }

    fn serve_with(proxy: &str, path: &str, host_port: &str, tcp: &str) -> String {
        format!(
            "{{\"TCP\": {{{tcp}}}, \"Web\": {{\"{host_port}\": {{\"Handlers\": \
             {{\"{path}\": {{\"Proxy\": \"{proxy}\"}}}}}}}}}}"
        )
    }

    #[test]
    fn every_loopback_spelling_counts_with_or_without_a_scheme() {
        for proxy in [
            "http://127.0.0.1:3890",
            "http://localhost:3890",
            "http://[::1]:3890",
            "127.0.0.1:3890",
            "localhost:3890",
            "[::1]:3890",
            "http://127.0.0.1:3890/",
            "http://LOCALHOST:3890",
        ] {
            let text = serve_with(
                proxy,
                "/",
                "box.tail.ts.net:443",
                "\"443\": {\"HTTPS\": true}",
            );
            assert_eq!(
                parse_serve_status_json(&text, 3890),
                Some(vec![route("https://box.tail.ts.net")]),
                "{proxy} reaches dux"
            );
        }
    }

    #[test]
    fn a_proxy_that_leaves_this_machine_or_speaks_tls_is_not_dux() {
        for proxy in [
            "http://192.168.1.5:3890",
            "http://100.101.102.103:3890",
            "http://example.com:3890",
            "https://127.0.0.1:3890",
            "https+insecure://127.0.0.1:3890",
            "http://127.0.0.1",
            "http://127.0.0.1:3890/sub",
            "",
        ] {
            let text = serve_with(
                proxy,
                "/",
                "box.tail.ts.net:443",
                "\"443\": {\"HTTPS\": true}",
            );
            assert_eq!(
                parse_serve_status_json(&text, 3890),
                Some(vec![]),
                "{proxy:?} does not end at dux"
            );
        }
    }

    #[test]
    fn a_handler_mounted_below_the_root_is_not_counted() {
        // dux's pages load their assets from the root, so a route that mounts it
        // under a path would serve a page that cannot load.
        let text = serve_with(
            "http://127.0.0.1:3890",
            "/dux",
            "box.tail.ts.net:443",
            "\"443\": {\"HTTPS\": true}",
        );
        assert_eq!(parse_serve_status_json(&text, 3890), Some(vec![]));
    }

    #[test]
    fn a_plain_http_serve_reads_as_http_and_default_ports_are_left_out() {
        let text = serve_with(
            "http://127.0.0.1:3890",
            "/",
            "box.tail.ts.net:80",
            "\"80\": {\"HTTP\": true}",
        );
        assert_eq!(
            parse_serve_status_json(&text, 3890),
            Some(vec![route("http://box.tail.ts.net")])
        );
        let text = serve_with(
            "http://127.0.0.1:3890",
            "/",
            "box.tail.ts.net:8080",
            "\"8080\": {\"HTTP\": true}",
        );
        assert_eq!(
            parse_serve_status_json(&text, 3890),
            Some(vec![route("http://box.tail.ts.net:8080")])
        );
    }

    #[test]
    fn a_funnelled_route_is_marked() {
        let text = "{\"TCP\": {\"443\": {\"HTTPS\": true}}, \"Web\": {\"box.tail.ts.net:443\": \
                    {\"Handlers\": {\"/\": {\"Proxy\": \"http://127.0.0.1:3890\"}}}}, \
                    \"AllowFunnel\": {\"box.tail.ts.net:443\": true}}";
        assert_eq!(
            parse_serve_status_json(text, 3890),
            Some(vec![ServeRoute {
                url: "https://box.tail.ts.net".to_string(),
                funnel: true,
            }])
        );
    }

    #[test]
    fn a_foreground_serve_counts_as_well_as_a_background_one() {
        // `tailscale serve 3890` without --bg keeps its config under a session.
        let text = "{\"Foreground\": {\"abc123\": {\"TCP\": {\"443\": {\"HTTPS\": true}}, \
                    \"Web\": {\"box.tail.ts.net:443\": {\"Handlers\": {\"/\": \
                    {\"Proxy\": \"http://127.0.0.1:3890\"}}}}}}}";
        assert_eq!(
            parse_serve_status_json(text, 3890),
            Some(vec![route("https://box.tail.ts.net")])
        );
    }

    #[test]
    fn several_routes_come_back_sorted_and_the_host_lowercased() {
        let text = "{\"TCP\": {\"443\": {\"HTTPS\": true}, \"8443\": {\"HTTPS\": true}}, \
                    \"Web\": {\
                      \"Box.Tail.ts.net:8443\": {\"Handlers\": {\"/\": {\"Proxy\": \"http://127.0.0.1:3890\"}}},\
                      \"box.tail.ts.net:443\": {\"Handlers\": {\"/\": {\"Proxy\": \"http://127.0.0.1:3890\"}}}\
                    }}";
        assert_eq!(
            parse_serve_status_json(text, 3890),
            Some(vec![
                route("https://box.tail.ts.net"),
                route("https://box.tail.ts.net:8443"),
            ])
        );
    }

    // ── The bounded probe, against a stand-in CLI ───────────────────────────

    /// A stand-in that answers `status --json --peers=false` and
    /// `serve status --json` with the given documents, and fails anything else,
    /// so a probe that asked with the wrong arguments fails the test.
    fn identity_cli(name: &str, status: &str, serve: &str) -> super::tests::StandIn {
        super::tests::StandIn::new(
            name,
            &format!(
                "if [ \"$*\" = \"status --json --peers=false\" ]; then printf '%s' '{status}'; \
                 elif [ \"$*\" = \"serve status --json\" ]; then printf '%s' '{serve}'; \
                 else exit 3; fi"
            ),
        )
    }

    #[test]
    fn the_probe_reads_the_name_and_the_route_from_the_two_commands() {
        let cli = identity_cli("identity-both", STATUS, SERVE);
        let identity =
            detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890).expect("both answered");
        assert_eq!(
            identity.status.dns_name.as_deref(),
            Some("demo-box.example-tailnet.ts.net")
        );
        assert_eq!(
            identity.serve,
            vec![route("https://demo-box.example-tailnet.ts.net:8443")]
        );
    }

    #[test]
    fn a_serve_status_that_fails_makes_the_whole_look_a_failure() {
        let cli = super::tests::StandIn::new(
            "identity-serve-fails",
            &format!("if [ \"$1\" = \"status\" ]; then printf '%s' '{STATUS}'; else exit 1; fi"),
        );
        assert_eq!(
            detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890),
            Err(TailscaleUnavailable::CommandFailed)
        );
    }

    #[test]
    fn garbage_from_either_command_is_no_answer() {
        let cli = identity_cli("identity-garbage", "not json", SERVE);
        assert_eq!(
            detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890),
            Err(TailscaleUnavailable::NoAddress)
        );
        let cli = identity_cli("identity-garbage-serve", STATUS, "No serve config");
        assert_eq!(
            detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890),
            Err(TailscaleUnavailable::NoAddress)
        );
    }

    #[test]
    fn a_missing_cli_is_missing() {
        assert_eq!(
            detect_identity_with("dux-no-such-tailscale-7c1e", DETECT_TIMEOUT, 3890),
            Err(TailscaleUnavailable::CommandMissing)
        );
    }

    #[test]
    fn a_wedged_status_command_is_cut_off_at_the_cap() {
        // Only `status` hangs, so the stand-in's readiness check (which runs it
        // with `ip`) returns at once and the cap is the only way out of the call.
        let cli = super::tests::StandIn::new(
            "identity-wedged",
            "if [ \"$1\" = \"status\" ]; then exec sleep 30; fi",
        );
        let cap = std::time::Duration::from_millis(300);
        let start = std::time::Instant::now();
        assert_eq!(
            detect_identity_with(cli.program(), cap, 3890),
            Err(TailscaleUnavailable::CommandFailed)
        );
        let elapsed = start.elapsed();
        assert!(elapsed >= cap, "ran into the cap: {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "did not wait out the sleep: {elapsed:?}"
        );
    }

    // ── Funnel, the security gate ───────────────────────────────────────────

    #[test]
    fn funnel_on_anything_at_all_counts_whatever_it_forwards_to() {
        let shapes = [
            // A route to dux through the Tailscale address, not loopback.
            "{\"Web\": {\"demo-box.example-tailnet.ts.net:443\": {\"Handlers\": {\"/\": \
             {\"Proxy\": \"http://100.101.102.103:3890\"}}}}, \
             \"AllowFunnel\": {\"demo-box.example-tailnet.ts.net:443\": true}}",
            // To a LAN address, and to every address.
            "{\"Web\": {\"h:443\": {\"Handlers\": {\"/\": {\"Proxy\": \"http://192.168.1.5:3890\"}}}}, \
             \"AllowFunnel\": {\"h:443\": true}}",
            "{\"Web\": {\"h:443\": {\"Handlers\": {\"/\": {\"Proxy\": \"http://0.0.0.0:3890\"}}}}, \
             \"AllowFunnel\": {\"h:443\": true}}",
            // Mounted under a path.
            "{\"Web\": {\"h:443\": {\"Handlers\": {\"/dux\": {\"Proxy\": \"http://127.0.0.1:3890\"}}}}, \
             \"AllowFunnel\": {\"h:443\": true}}",
            // A raw TCP forward, and a TLS-terminated one.
            "{\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:3890\"}}, \
             \"AllowFunnel\": {\"h:443\": true}}",
            "{\"TCP\": {\"8443\": {\"TCPForward\": \"127.0.0.1:3890\", \"TerminateTLS\": \"h\"}}, \
             \"AllowFunnel\": {\"h:8443\": true}}",
            // Something else entirely on another port.
            "{\"Web\": {\"h:10000\": {\"Handlers\": {\"/\": {\"Proxy\": \"http://127.0.0.1:9000\"}}}}, \
             \"AllowFunnel\": {\"h:10000\": true}}",
            // Inside a foreground session.
            "{\"Foreground\": {\"s1\": {\"AllowFunnel\": {\"h:443\": true}}}}",
            // One true among falses.
            "{\"AllowFunnel\": {\"h:443\": false, \"h:8443\": true}}",
        ];
        for shape in shapes {
            assert_eq!(parse_serve_funnel(shape), Some(true), "{shape}");
        }
    }

    #[test]
    fn no_funnel_entry_or_only_false_ones_is_no_funnel() {
        assert_eq!(parse_serve_funnel(SERVE), Some(false));
        assert_eq!(parse_serve_funnel("{}"), Some(false));
        assert_eq!(
            parse_serve_funnel("{\"AllowFunnel\": {\"h:443\": false}}"),
            Some(false)
        );
        assert_eq!(parse_serve_funnel("not json"), None);
        assert_eq!(parse_serve_funnel("[]"), None);
    }

    #[test]
    fn the_probe_reports_funnel_on_any_route() {
        let serve = "{\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:22\"}}, \
                     \"AllowFunnel\": {\"demo-box.example-tailnet.ts.net:443\": true}}";
        let cli = identity_cli("identity-funnel", STATUS, serve);
        let identity = detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890).unwrap();
        assert!(identity.funnel);
        assert!(
            identity.serve.is_empty(),
            "nothing there reaches dux to show"
        );
    }

    // ── A raw TCP Funnel to dux ─────────────────────────────────────────────

    fn tcp_funnel(target: &str, terminate_tls: bool) -> String {
        let tls = if terminate_tls {
            ", \"TerminateTLS\": \"demo-box.example-tailnet.ts.net\""
        } else {
            ""
        };
        format!(
            "{{\"TCP\": {{\"443\": {{\"TCPForward\": \"{target}\"{tls}}}}}, \
             \"AllowFunnel\": {{\"demo-box.example-tailnet.ts.net:443\": true}}}}"
        )
    }

    #[test]
    fn a_funnelled_tcp_forward_to_dux_counts_whatever_address_it_names() {
        for target in [
            "127.0.0.1:3890",
            "localhost:3890",
            "[::1]:3890",
            "100.101.102.103:3890",
            "0.0.0.0:3890",
            "[::]:3890",
            "192.168.1.5:3890",
        ] {
            for tls in [false, true] {
                assert_eq!(
                    parse_funnel_to_port(&tcp_funnel(target, tls), 3890),
                    Some(true),
                    "{target} tls={tls}"
                );
            }
        }
    }

    #[test]
    fn a_tcp_forward_that_is_not_funnelled_or_not_to_dux_does_not_count() {
        assert_eq!(
            parse_funnel_to_port(&tcp_funnel("127.0.0.1:22", false), 3890),
            Some(false)
        );
        let tailnet_only = "{\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:3890\"}}}";
        assert_eq!(parse_funnel_to_port(tailnet_only, 3890), Some(false));
        let other_port_funnelled = "{\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:3890\"}}, \
                                    \"AllowFunnel\": {\"h:8443\": true}}";
        // Deliberately conservative: a Funnel on any port with a forward to dux
        // on any port locks out, because Tailscale pairs them across configs.
        assert_eq!(parse_funnel_to_port(other_port_funnelled, 3890), Some(true));
        // A web route through Funnel to dux counts as well: the proxy forwards
        // the client's own Host, and an older daemon sends no Funnel marker.
        let web = "{\"TCP\": {\"443\": {\"HTTPS\": true}}, \"Web\": {\"h:443\": {\"Handlers\": \
                   {\"/\": {\"Proxy\": \"http://127.0.0.1:3890\"}}}}, \"AllowFunnel\": {\"h:443\": true}}";
        assert_eq!(parse_funnel_to_port(web, 3890), Some(true));
        assert_eq!(parse_funnel_to_port("{}", 3890), Some(false));
        assert_eq!(parse_funnel_to_port("nope", 3890), None);
    }

    fn web_funnel(proxy: &str, path: &str) -> String {
        format!(
            "{{\"TCP\": {{\"443\": {{\"HTTPS\": true}}}}, \"Web\": {{\"h:443\": {{\"Handlers\": \
             {{\"{path}\": {{\"Proxy\": \"{proxy}\"}}}}}}}}, \"AllowFunnel\": {{\"h:443\": true}}}}"
        )
    }

    #[test]
    fn a_funnelled_web_handler_to_dux_counts_whatever_host_path_or_scheme() {
        // The serve proxy forwards the public client's own Host, so a web route
        // through Funnel can claim `localhost`: any web handler to dux's port
        // has to count, not only the ones dux would show.
        for proxy in [
            "http://127.0.0.1:3890",
            "http://localhost:3890",
            "http://100.101.102.103:3890",
            "http://0.0.0.0:3890",
            "http://192.168.1.5:3890",
            "https+insecure://127.0.0.1:3890",
            "127.0.0.1:3890",
            "3890",
        ] {
            for path in ["/", "/dux"] {
                assert_eq!(
                    parse_funnel_to_port(&web_funnel(proxy, path), 3890),
                    Some(true),
                    "{proxy} at {path}"
                );
            }
        }
        assert_eq!(
            parse_funnel_to_port(&web_funnel("http://127.0.0.1:9000", "/"), 3890),
            Some(false),
            "another port is not dux"
        );
        let tailnet_only = web_funnel("http://127.0.0.1:3890", "/").replace("true}}", "false}}");
        assert_eq!(parse_funnel_to_port(&tailnet_only, 3890), Some(false));
    }

    #[test]
    fn a_daemon_that_is_not_running_is_told_apart_from_a_failing_cli() {
        // What the CLI says when its socket is not there.
        let cli = super::tests::StandIn::new(
            "daemon-down",
            "if [ \"$1\" = \"status\" ]; then echo 'Failed to connect to local Tailscale \
             daemon for /localapi/v0/status; not running? Error: dial unix: connect: no such \
             file or directory' >&2; exit 1; fi",
        );
        assert_eq!(
            detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890),
            Err(TailscaleUnavailable::DaemonUnreachable)
        );
        let failing = super::tests::StandIn::new(
            "cli-fails",
            "if [ \"$1\" = \"status\" ]; then echo 'some other error' >&2; exit 1; fi",
        );
        assert_eq!(
            detect_identity_with(failing.program(), DETECT_TIMEOUT, 3890),
            Err(TailscaleUnavailable::CommandFailed)
        );
    }

    // ── Funnel and a forward split across configs ───────────────────────────

    #[test]
    fn a_funnel_and_a_forward_to_dux_lock_out_wherever_each_one_sits() {
        // Tailscale pairs an AllowFunnel from the top level or any foreground
        // session with a handler from any of them, so the two halves can live
        // apart. `--tcp 443 off` leaves the top-level AllowFunnel behind.
        let shapes = [
            // Top-level AllowFunnel, foreground TCP forward to dux.
            "{\"AllowFunnel\": {\"h:443\": true}, \"Foreground\": {\"s\": \
             {\"TCP\": {\"443\": {\"TCPForward\": \"localhost:3890\"}}}}}",
            // Foreground AllowFunnel, top-level web proxy to dux.
            "{\"Web\": {\"h:443\": {\"Handlers\": {\"/\": {\"Proxy\": \"http://127.0.0.1:3890\"}}}}, \
             \"Foreground\": {\"s\": {\"AllowFunnel\": {\"h:443\": true}}}}",
            // Funnel on one port, the forward to dux on another.
            "{\"AllowFunnel\": {\"h:443\": true}, \"TCP\": {\"8443\": {\"TCPForward\": \"127.0.0.1:3890\"}}}",
            // Two foreground sessions, one half each.
            "{\"Foreground\": {\"a\": {\"AllowFunnel\": {\"h:443\": true}}, \
             \"b\": {\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:3890\"}}}}}",
        ];
        for shape in shapes {
            assert_eq!(parse_funnel_to_port(shape, 3890), Some(true), "{shape}");
        }
        // Nothing funnelled, or nothing reaching dux, is not a lockout.
        let no_funnel = "{\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:3890\"}}}";
        assert_eq!(parse_funnel_to_port(no_funnel, 3890), Some(false));
        let not_dux = "{\"AllowFunnel\": {\"h:443\": true}, \
                       \"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:22\"}}}";
        assert_eq!(parse_funnel_to_port(not_dux, 3890), Some(false));
    }

    #[test]
    fn a_target_without_a_port_names_its_schemes_default() {
        let at = |proxy: &str| {
            format!(
                "{{\"AllowFunnel\": {{\"h:443\": true}}, \"Web\": {{\"h:443\": {{\"Handlers\": \
                 {{\"/\": {{\"Proxy\": \"{proxy}\"}}}}}}}}}}"
            )
        };
        assert_eq!(
            parse_funnel_to_port(&at("http://127.0.0.1"), 80),
            Some(true)
        );
        assert_eq!(
            parse_funnel_to_port(&at("http://127.0.0.1/"), 80),
            Some(true)
        );
        assert_eq!(
            parse_funnel_to_port(&at("https://127.0.0.1"), 443),
            Some(true)
        );
        assert_eq!(
            parse_funnel_to_port(&at("http://127.0.0.1"), 3890),
            Some(false)
        );
    }

    // ── Finding and reading the CLI ─────────────────────────────────────────

    #[test]
    fn a_cli_that_rejects_the_peers_flag_is_asked_again_without_it() {
        let cli = super::tests::StandIn::new(
            "no-peers-flag",
            &format!(
                "if [ \"$*\" = \"status --json --peers=false\" ]; then \
                 echo 'flag provided but not defined: -peers' >&2; exit 2; \
                 elif [ \"$*\" = \"status --json\" ]; then printf '%s' '{STATUS}'; \
                 elif [ \"$*\" = \"serve status --json\" ]; then printf '%s' '{SERVE}'; \
                 else exit 3; fi"
            ),
        );
        let identity = detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890).unwrap();
        assert_eq!(
            identity.status.dns_name.as_deref(),
            Some("demo-box.example-tailnet.ts.net")
        );
    }

    #[test]
    fn a_cli_built_without_serve_has_no_funnel_to_report() {
        let cli = super::tests::StandIn::new(
            "no-serve",
            &format!(
                "if [ \"$1\" = \"status\" ]; then printf '%s' '{STATUS}'; \
                 else echo 'tailscale: unknown subcommand: serve' >&2; exit 1; fi"
            ),
        );
        let identity = detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890).unwrap();
        assert!(!identity.funnel && !identity.funnel_to_dux);
        assert!(identity.serve.is_empty());

        // Any other failure of `serve status` stays a failure.
        let failing = super::tests::StandIn::new(
            "serve-fails",
            &format!(
                "if [ \"$1\" = \"status\" ]; then printf '%s' '{STATUS}'; \
                 else echo 'something else broke' >&2; exit 1; fi"
            ),
        );
        assert_eq!(
            detect_identity_with(failing.program(), DETECT_TIMEOUT, 3890),
            Err(TailscaleUnavailable::CommandFailed)
        );
    }

    #[test]
    fn the_cli_is_looked_for_where_tailscale_installs_it_when_path_has_none() {
        let cli = identity_cli("found-second", STATUS, SERVE);
        let identity = detect_identity_checked(
            &["dux-no-such-tailscale-1a2b", cli.program()],
            DETECT_TIMEOUT,
            3890,
            &|| panic!("a CLI was found, so the interfaces are never asked"),
        )
        .unwrap();
        assert_eq!(
            identity.status.dns_name.as_deref(),
            Some("demo-box.example-tailnet.ts.net")
        );
        assert!(
            CLI_CANDIDATES.contains(&"/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
            "the macOS app bundle's CLI, from Tailscale's docs"
        );
        assert_eq!(CLI_CANDIDATES[0], "tailscale", "PATH first");
    }

    #[test]
    fn no_cli_means_no_funnel_only_when_this_machine_has_no_tailscale_address() {
        let missing = ["dux-no-such-tailscale-3c4d"];
        assert_eq!(
            detect_identity_checked(&missing, DETECT_TIMEOUT, 3890, &|| false),
            Err(TailscaleUnavailable::CommandMissing)
        );
        assert_eq!(
            detect_identity_checked(&missing, DETECT_TIMEOUT, 3890, &|| true),
            Err(TailscaleUnavailable::Unverifiable),
            "a Tailscale address with no way to ask about its Funnels is unknown"
        );
        let unreachable = super::tests::StandIn::new(
            "dial-error",
            "if [ \"$1\" = \"status\" ]; then echo 'Failed to connect to local Tailscale \
             daemon for /localapi/v0/status; not running? Error: dial unix: permission denied' \
             >&2; exit 1; fi",
        );
        assert_eq!(
            detect_identity_checked(&[unreachable.program()], DETECT_TIMEOUT, 3890, &|| false),
            Err(TailscaleUnavailable::DaemonUnreachable)
        );
        assert_eq!(
            detect_identity_checked(&[unreachable.program()], DETECT_TIMEOUT, 3890, &|| true),
            Err(TailscaleUnavailable::Unverifiable),
            "a dial error is not proof the daemon is gone (a permission, a wrong socket)"
        );
    }

    #[test]
    fn a_tailscale_address_is_recognised_in_either_range_and_nothing_else() {
        let ips =
            |list: &[&str]| -> Vec<IpAddr> { list.iter().map(|s| s.parse().unwrap()).collect() };
        assert!(has_tailscale_address(&ips(&[
            "127.0.0.1",
            "100.101.102.103"
        ])));
        assert!(has_tailscale_address(&ips(&["fd7a:115c:a1e0::1"])));
        assert!(!has_tailscale_address(&ips(&[
            "127.0.0.1",
            "192.168.1.5",
            "fe80::1"
        ])));
        assert!(!has_tailscale_address(&[]));
    }

    #[test]
    fn this_machines_interface_addresses_can_be_read() {
        let addrs = interface_addresses();
        assert!(
            addrs.iter().any(|ip| ip.is_loopback()),
            "every machine dux runs on has a loopback address: {addrs:?}"
        );
    }

    #[test]
    fn a_funnelled_tcp_forward_in_a_foreground_session_counts() {
        let text = "{\"Foreground\": {\"s1\": {\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:3890\"}}, \
                    \"AllowFunnel\": {\"h:443\": true}}}}";
        assert_eq!(parse_funnel_to_port(text, 3890), Some(true));
    }

    #[test]
    fn the_probe_reports_a_tcp_funnel_to_dux() {
        let cli = identity_cli(
            "identity-tcp-funnel",
            STATUS,
            &tcp_funnel("127.0.0.1:3890", false),
        );
        let identity = detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890).unwrap();
        assert!(identity.funnel_to_dux);
        assert!(identity.funnel);
    }

    // ── A name Tailscale assigned ───────────────────────────────────────────

    fn status_named(name: &str, suffix: Option<&str>) -> SelfStatus {
        SelfStatus {
            dns_name: Some(name.to_string()),
            magic_dns_enabled: true,
            magic_dns_suffix: suffix.map(str::to_string),
            cert_domains: Vec::new(),
        }
    }

    #[test]
    fn only_a_name_under_the_tailnets_ts_net_suffix_is_tailscale_assigned() {
        let real = parse_status_json(STATUS).unwrap();
        assert_eq!(
            real.tailscale_assigned_name(),
            Some("demo-box.example-tailnet.ts.net")
        );
        // A Headscale (or other control server) domain the operator chose.
        assert_eq!(
            status_named("box.vpn.example.com", Some("vpn.example.com")).tailscale_assigned_name(),
            None
        );
        // A name outside the suffix the daemon reports.
        assert_eq!(
            status_named("box.other.ts.net", Some("example-tailnet.ts.net"))
                .tailscale_assigned_name(),
            None
        );
        // No suffix reported at all, the bare suffix, and a look-alike.
        assert_eq!(
            status_named("box.example-tailnet.ts.net", None).tailscale_assigned_name(),
            None
        );
        assert_eq!(
            status_named("example-tailnet.ts.net", Some("example-tailnet.ts.net"))
                .tailscale_assigned_name(),
            None
        );
        assert_eq!(
            status_named("boxexample-tailnet.ts.net", Some("example-tailnet.ts.net"))
                .tailscale_assigned_name(),
            None
        );
    }

    #[test]
    fn fields_of_the_wrong_type_are_skipped_rather_than_failing_the_whole_answer() {
        let text = "{\"TCP\": 5, \"Web\": {\"box.tail.ts.net:443\": {\"Handlers\": {\"/\": \
                    {\"Proxy\": 7}}}, \"other:443\": \"nope\"}}";
        assert_eq!(parse_serve_status_json(text, 3890), Some(vec![]));
    }
}
