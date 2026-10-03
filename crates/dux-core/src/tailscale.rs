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
    /// The CLI ran and said it could not reach the Tailscale daemon, in words
    /// that do not prove it stopped (a socket it cannot use, a process it
    /// found). Unknown.
    DaemonUnreachable,
    /// The CLI ran and said, in its own words for it, that no daemon is running
    /// (see [`daemon_stopped_message`]). Funnel needs a running daemon, so this
    /// is an answer: nothing is published.
    DaemonStopped,
    /// The CLI is missing everywhere dux looks, yet something of Tailscale is
    /// here: an address on Tailscale's own interface, a daemon socket that
    /// answers, or a running `tailscaled` (userspace networking has no
    /// interface at all). A daemon may be up that nothing here can ask about
    /// its Funnels. Unknown, never "no Funnel"; the way out is the CLI.
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
            Self::DaemonStopped => "the Tailscale daemon is not running",
            Self::Unverifiable => {
                "Tailscale is on this machine (an address on its interface, a daemon socket, \
                 or a running Tailscale daemon), but the tailscale CLI is not where dux looks"
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
/// "serve loopback only" and warns. A CLI that is not there maps to
/// `CommandMissing`, any other launch failure, a timeout or a non-zero exit to
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
    let path = std::env::var_os("PATH");
    for program in programs {
        if !cli_on_disk(program, path.as_deref()) {
            continue;
        }
        last = ask(program);
        if !matches!(last, Err(TailscaleUnavailable::CommandMissing)) {
            return last;
        }
    }
    last
}

/// Every address on this machine's network interfaces with the interface's
/// name, read with `getifaddrs`. Empty when they cannot be read.
pub fn interfaces() -> Vec<(String, IpAddr)> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` fills `list` with a linked list it allocated, which
    // is walked read-only below and handed back to `freeifaddrs` exactly once.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: `cursor` is a node of the list `getifaddrs` returned and has
        // not been freed; `ifa_name` is a NUL-terminated string it owns, and
        // `ifa_addr` is null or points at a sockaddr whose family field says
        // which concrete type it is.
        unsafe {
            let entry = &*cursor;
            let sockaddr = entry.ifa_addr;
            let name = if entry.ifa_name.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(entry.ifa_name)
                    .to_string_lossy()
                    .into_owned()
            };
            if !sockaddr.is_null() {
                match i32::from((*sockaddr).sa_family) {
                    libc::AF_INET => {
                        let v4 = &*(sockaddr as *const libc::sockaddr_in);
                        found.push((
                            name,
                            IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr))),
                        ));
                    }
                    libc::AF_INET6 => {
                        let v6 = &*(sockaddr as *const libc::sockaddr_in6);
                        found.push((
                            name,
                            IpAddr::V6(std::net::Ipv6Addr::from(v6.sin6_addr.s6_addr)),
                        ));
                    }
                    _ => {}
                }
            }
            cursor = entry.ifa_next;
        }
    }
    // SAFETY: `list` came from a successful `getifaddrs` and is freed once.
    unsafe { libc::freeifaddrs(list) };
    found
}

/// Whether one of `ifaces` holds a Tailscale address on TAILSCALE's own
/// interface: one named `tailscale*` (Linux's `tailscale0`), or any interface
/// (a macOS `utun`, a custom tun name) carrying an address Tailscale itself
/// reported in `known`. A bare 100.64.0.0/10 address is not enough: Cloudflare
/// WARP (100.96.0.0/12) and NetBird use the same block.
pub fn owned_tailscale_address(ifaces: &[(String, IpAddr)], known: &[IpAddr]) -> bool {
    ifaces.iter().any(|(name, ip)| {
        let in_range = match ip {
            IpAddr::V4(v4) => is_tailscale_cgnat(*v4),
            IpAddr::V6(v6) => is_tailscale_ipv6(*v6),
        };
        in_range && (name.starts_with("tailscale") || known.contains(ip))
    })
}

/// Where a `tailscaled` keeps its LocalAPI socket by default (Tailscale's
/// `paths.DefaultTailscaledSocket` and the official container image), checked
/// so a daemon running in userspace networking, which has no interface, is
/// still seen. `$TS_SOCKET` is checked as well.
pub const SOCKET_PATHS: &[&str] = &[
    "/var/run/tailscale/tailscaled.sock",
    "/run/tailscale/tailscaled.sock",
    "/var/run/tailscaled.socket",
    "/tmp/tailscaled.sock",
    "/var/packages/Tailscale/var/tailscaled.sock",
    "/var/packages/Tailscale/etc/tailscaled.sock",
    "/tmp/tailscale/tailscaled.sock",
    "/perm/tailscaled/tailscaled.sock",
];

/// The process names of a running Tailscale daemon: `tailscaled`, the name
/// the CLI itself looks for on macOS (`IPNExtension`, cmd/tailscale/cli/diag.go),
/// and the network extensions of the macOS Standalone and App Store apps, whose
/// executables are named after their bundle ids (version/prop.go,
/// `macsysExtBundleId` and `appStoreExtBundleId`). The apps' CLI cannot always
/// reach a running app, and a running extension must read as "cannot check",
/// never as "stopped".
pub const PROCESS_NAMES: &[&str] = &[
    "tailscaled",
    "IPNExtension",
    "io.tailscale.ipn.macsys.network-extension",
    "io.tailscale.ipn.macos.network-extension",
];

/// The shortest name a process table cuts a long name to: Linux keeps 15
/// bytes of `comm`, macOS 16 of `p_comm`.
const SHORTEST_CUT_PROCESS_NAME: usize = 15;

/// Whether a process table's `name` is one of [`PROCESS_NAMES`], or one of
/// them cut short by the table. A cut prefix can match an unrelated process
/// that happens to share it; that only ever refuses more.
fn is_tailscale_process_name(name: &str) -> bool {
    PROCESS_NAMES.iter().any(|wanted| {
        name == *wanted
            || (name.len() >= SHORTEST_CUT_PROCESS_NAME
                && name.len() < wanted.len()
                && wanted.starts_with(name))
    })
}

/// How long a process table that showed no Tailscale daemon is trusted before
/// it is read again. Reading it is by far the dearest part of a look on a
/// machine without Tailscale (tens of milliseconds, against well under one for
/// the interfaces and sockets), and a look runs every few seconds for the
/// whole serve. Every cheaper signal (the CLI on disk, an address on
/// Tailscale's interface, a daemon socket) is still read every look, so only a
/// daemon with no socket at a known path and no interface (userspace
/// networking with a custom socket) waits up to this long to be seen.
pub const PROCESS_RESCAN: std::time::Duration = std::time::Duration::from_secs(60);

/// The last process-table reading, so an empty one is reused for
/// [`PROCESS_RESCAN`]. A table that showed a daemon is read again every time,
/// so its going is noticed at once.
#[derive(Debug, Default)]
struct ProcessScanCache {
    empty_at: Option<std::time::Instant>,
}

impl ProcessScanCache {
    const fn new() -> Self {
        Self { empty_at: None }
    }

    fn running(&mut self, now: std::time::Instant, scan: impl FnOnce() -> bool) -> bool {
        if self
            .empty_at
            .is_some_and(|at| now.saturating_duration_since(at) < PROCESS_RESCAN)
        {
            return false;
        }
        let running = scan();
        self.empty_at = (!running).then_some(now);
        running
    }
}

static PROCESS_SCAN: std::sync::Mutex<ProcessScanCache> =
    std::sync::Mutex::new(ProcessScanCache::new());

/// Whether `program` could be on disk: a path that exists, or a bare name found
/// in one of `path`'s directories. With no `PATH` to search, `true`: the system
/// is asked by running it instead. A look on a machine without Tailscale then
/// costs a few `stat`s rather than a spawn per candidate.
fn cli_on_disk(program: &str, path: Option<&std::ffi::OsStr>) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).exists();
    }
    match path {
        Some(path) => std::env::split_paths(path).any(|dir| dir.join(program).exists()),
        None => true,
    }
}

/// Whether a `tailscaled` is detectable without the CLI: its socket answers
/// (see [`socket_answers`]) at a known path or at `$TS_SOCKET`, or a
/// process with its name is running.
pub fn tailscaled_detectable(
    answers: &dyn Fn(&std::path::Path) -> bool,
    ts_socket: Option<String>,
    process_running: bool,
) -> bool {
    process_running
        || SOCKET_PATHS
            .iter()
            .any(|path| answers(std::path::Path::new(path)))
        || ts_socket
            .filter(|path| !path.is_empty())
            .is_some_and(|path| answers(std::path::Path::new(&path)))
}

/// Whether a daemon answers on the socket at `path`: one non-blocking
/// connect, which a local socket answers at once, so it needs no timeout. Only
/// "refused" (a file left behind by a daemon that is gone, or not a socket at
/// all) and "not found" mean nobody is there; counting the leftover would keep
/// dux refusing forever. Everything else counts as something there that dux
/// cannot ask: connected, a full accept queue (`EAGAIN`), a connect still in
/// progress, a permission refused, and any other error.
fn socket_answers(path: &std::path::Path) -> bool {
    let Ok(address) = socket2::SockAddr::unix(path) else {
        return false;
    };
    let Ok(socket) = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)
    else {
        return false;
    };
    if socket.set_nonblocking(true).is_err() {
        // Cannot ask without risking a wait: count it, as for any doubt.
        return true;
    }
    match socket.connect(&address) {
        Ok(()) => true,
        Err(err) => !matches!(
            err.raw_os_error(),
            Some(libc::ECONNREFUSED | libc::ENOENT | libc::ENOTDIR)
        ),
    }
}

/// Whether a process named like a Tailscale daemon is running right now.
fn tailscaled_process_running() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    system
        .processes()
        .values()
        .any(|process| is_tailscale_process_name(&process.name().to_string_lossy()))
}

/// What this machine shows of Tailscale without asking the CLI, read only when
/// the CLI could not answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalEvidence {
    /// A Tailscale address on Tailscale's own interface.
    pub owned_address: bool,
    /// A `tailscaled` socket or process.
    pub daemon_detectable: bool,
}

/// The addresses the last successful look reported for this machine, so a
/// later look that fails can still tell Tailscale's `utun` from another VPN's.
static KNOWN_TAILSCALE_IPS: std::sync::Mutex<Vec<IpAddr>> = std::sync::Mutex::new(Vec::new());

/// Read this machine's [`LocalEvidence`].
pub fn local_evidence() -> LocalEvidence {
    let known = KNOWN_TAILSCALE_IPS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    evidence_from(
        owned_tailscale_address(&interfaces(), &known),
        &socket_answers,
        std::env::var("TS_SOCKET").ok(),
        &|| {
            PROCESS_SCAN
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .running(std::time::Instant::now(), tailscaled_process_running)
        },
    )
}

/// [`LocalEvidence`] from the cheapest signal that answers: an owned address,
/// then a daemon socket, and the process table only when neither does. Only
/// "something of Tailscale is here" matters to the caller, so the first yes
/// ends the reading.
fn evidence_from(
    owned_address: bool,
    answers: &dyn Fn(&std::path::Path) -> bool,
    ts_socket: Option<String>,
    process_running: &dyn Fn() -> bool,
) -> LocalEvidence {
    if owned_address {
        return LocalEvidence {
            owned_address: true,
            daemon_detectable: false,
        };
    }
    let daemon_detectable = tailscaled_detectable(answers, ts_socket, false) || process_running();
    LocalEvidence {
        owned_address: false,
        daemon_detectable,
    }
}

/// Whether the CLI's error is one of its own words for a daemon that is not
/// running. `tailscale status` prints these only when it found no `tailscaled`
/// (or macOS `IPNExtension`) process AND no socket (cmd/tailscale/cli/diag.go,
/// `fixTailscaledConnectErrorImpl`); every other connect failure (a socket it
/// cannot use, a process it found) is not proof of anything.
pub fn daemon_stopped_message(stderr: &str) -> bool {
    let text = stderr.to_ascii_lowercase();
    [
        "failed to connect to local tailscaled; it doesn't appear to be running",
        "failed to connect to local tailscale service; is tailscale running?",
        "failed to connect to local tailscaled process; it doesn't appear to be running",
    ]
    .iter()
    .any(|wording| text.contains(wording))
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
    /// This machine's Tailscale addresses (`Self.TailscaleIPs`), which is how a
    /// later failed look tells Tailscale's interface from another VPN's.
    pub tailscale_ips: Vec<IpAddr>,
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
    /// Whether the node is down (`Stopped`, `NeedsLogin`, `NeedsMachineAuth`),
    /// so a saved Funnel to dux comes back only once it is brought up.
    pub node_down: bool,
}

/// Ask the `tailscale` CLI for this machine's name and the serve routes that
/// end at `dux_port`. Bounded exactly like [`detect_ip`]; see
/// [`detect_identity_with`].
pub fn detect_identity(dux_port: u16) -> Result<TailscaleIdentity, TailscaleUnavailable> {
    let look = detect_identity_checked(CLI_CANDIDATES, DETECT_TIMEOUT, dux_port, &local_evidence);
    if let Ok(identity) = &look {
        *KNOWN_TAILSCALE_IPS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            identity.status.tailscale_ips.clone();
    }
    look
}

/// [`detect_identity`] over a list of candidate programs, with this machine's
/// [`LocalEvidence`] injected (read only when the CLI could not answer).
///
/// The strict rule: dux serves only on an ANSWER. A CLI that is missing
/// everywhere is an answer ("no Tailscale here") only when nothing of Tailscale
/// is here: no address on Tailscale's own interface and no daemon socket or
/// process. A daemon the CLI says is not running is an answer ("no Funnel")
/// only on the same terms. Anything else is
/// [`TailscaleUnavailable::Unverifiable`].
pub fn detect_identity_checked(
    programs: &[&str],
    timeout: std::time::Duration,
    dux_port: u16,
    evidence: &dyn Fn() -> LocalEvidence,
) -> Result<TailscaleIdentity, TailscaleUnavailable> {
    match first_cli_answer(programs, |program| {
        detect_identity_with(program, timeout, dux_port)
    }) {
        Err(
            reason @ (TailscaleUnavailable::CommandMissing | TailscaleUnavailable::DaemonStopped),
        ) => {
            let here = evidence();
            if !(here.owned_address || here.daemon_detectable) {
                Err(reason)
            } else if reason == TailscaleUnavailable::CommandMissing {
                Err(TailscaleUnavailable::Unverifiable)
            } else {
                // The CLI says stopped, yet a daemon shows: one it cannot reach.
                Err(TailscaleUnavailable::DaemonUnreachable)
            }
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
    let down = backend_cannot_serve(&status);
    let status = parse_status_json(&status).ok_or(TailscaleUnavailable::NoAddress)?;
    // A CLI built without `serve` (`ts_omit_serve`) has a daemon that cannot
    // serve, so it has no Funnel to report; any other failure stays one.
    let serve = match run_cli_raw(program, &["serve", "status", "--json"], timeout) {
        Ok(serve) => serve,
        Err(failure) if failure.stderr.contains("unknown subcommand") => "{}".to_string(),
        Err(failure) => return Err(failure.kind),
    };
    // `serve status --json` marshals a nil config, which is what a node that
    // never served has, as `null`.
    let serve = if serve.trim() == "null" {
        "{}".to_string()
    } else {
        serve
    };
    let funnel = parse_serve_funnel(&serve).ok_or(TailscaleUnavailable::NoAddress)?;
    let funnel_to_dux =
        parse_funnel_to_port(&serve, dux_port).ok_or(TailscaleUnavailable::NoAddress)?;
    let serve = parse_serve_status_json(&serve, dux_port).ok_or(TailscaleUnavailable::NoAddress)?;
    if down {
        // A node that is down publishes nothing now, but its saved config is
        // what `tailscale up` brings back: a Funnel to dux keeps the lockout,
        // and nothing else is reported (no Funnel to withdraw a name for, no
        // route to show).
        return Ok(TailscaleIdentity {
            status,
            serve: Vec::new(),
            funnel: false,
            funnel_to_dux,
            node_down: true,
        });
    }
    Ok(TailscaleIdentity {
        status,
        serve,
        funnel,
        funnel_to_dux,
        node_down: false,
    })
}

/// Whether `tailscale status --json` reports a node that publishes nothing right
/// now: `tailscale down` (`Stopped`), logged out (`NeedsLogin`), or waiting on
/// an administrator's approval (`NeedsMachineAuth`). Each needs a person to act
/// before the node rejoins the tailnet, and Funnel traffic arrives over the
/// tailnet. Its saved serve configuration still decides the Funnel lockout,
/// because that is what comes back with it. `Starting` and `NoState` are a
/// daemon on its way up, read exactly like `Running`.
fn backend_cannot_serve(status: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(status)
        .ok()
        .and_then(|value| {
            value
                .get("BackendState")
                .and_then(|state| state.as_str())
                .map(|state| matches!(state, "Stopped" | "NeedsLogin" | "NeedsMachineAuth"))
        })
        .unwrap_or(false)
}

/// Run one bounded `tailscale` CLI call and hand back its stdout. A program
/// that is not there is a missing CLI; any other launch failure, a timeout or a
/// non-zero exit is a failure.
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
    // The macOS app's bundled executable runs as the CLI only when it thinks it
    // is in a shell, or when this is set (Tailscale's macOS CLI docs). Set on
    // every call, so it never opens or focuses the app; every other CLI
    // ignores it.
    cmd.env("TAILSCALE_BE_CLI", "1");
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
        // Only "not found" says the CLI is absent. A permission refused, a
        // resource exhausted or a failed wait says nothing about what is
        // installed, so it is a failure: dux cannot check.
        crate::bounded_command::CommandOutcome::NotFound(err) => {
            logger::debug(&format!("[tailscale] `{program}` is not there: {err}"));
            return Err(TailscaleUnavailable::CommandMissing.into());
        }
        crate::bounded_command::CommandOutcome::Failed(err) => {
            logger::debug(&format!(
                "[tailscale] could not run `{program} {shown}`: {err}"
            ));
            return Err(TailscaleUnavailable::CommandFailed.into());
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
        let kind = if daemon_stopped_message(&stderr) {
            TailscaleUnavailable::DaemonStopped
        } else if stderr
            .to_ascii_lowercase()
            .contains("failed to connect to local tailscale")
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

/// Whether a Funnel publishes anything that forwards to `dux_port`. Paired the
/// way Tailscale pairs them (ipn/serve.go): each `host:port` an `AllowFunnel`
/// switches on, at the top level or in any foreground session
/// (`HasFunnelForTarget`), with the handler Tailscale would use for that same
/// `host:port`: a foreground session's before the top level's (`FindTCP`,
/// `FindWeb`). It counts when that handler is a `TCPForward` (TLS-terminated
/// or not) or a `Web` handler (any path) whose `Proxy` names dux's port. A
/// Funnel for another port, even one with a forward to dux elsewhere in the
/// config, publishes nothing of dux. `None` when the text is not a JSON
/// object.
///
/// Where Tailscale's order is not knowable it errs toward counting: every
/// foreground session's handler for the port counts, since Go walks that map
/// in no fixed order; an `AllowFunnel` key whose port cannot be read pairs
/// with every handler; and when no `AllowFunnel` at the top level or in a
/// foreground session switches anything on, yet one somewhere else in the
/// document does (a shape this code does not know), the answer falls back to
/// "any Funnel and any forward to dux". A target on a Unix socket (`unix:`) is
/// never dux, which only listens on TCP.
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
    let top = value.as_object()?;
    let foreground: Vec<&serde_json::Map<String, serde_json::Value>> = top
        .get("Foreground")
        .and_then(|sessions| sessions.as_object())
        .map(|sessions| sessions.values().filter_map(|s| s.as_object()).collect())
        .unwrap_or_default();
    let configs: Vec<&serde_json::Map<String, serde_json::Value>> = std::iter::once(top)
        .chain(foreground.iter().copied())
        .collect();
    let targets: Vec<&str> = configs
        .iter()
        .filter_map(|config| config.get("AllowFunnel").and_then(|a| a.as_object()))
        .flat_map(|allowed| {
            allowed
                .iter()
                .filter(|(_, on)| on.as_bool() == Some(true))
                .map(|(host_port, _)| host_port.as_str())
        })
        .collect();
    if targets.is_empty() {
        // A Funnel switched on somewhere this code does not know: the
        // conservative reading.
        return Some(any_funnel(&value) && any_target_to_port(&value, dux_port));
    }
    Some(targets.iter().any(|target| {
        let Some(port) = target
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
        else {
            return any_target_to_port(&value, dux_port);
        };
        let names_dux = |target: Option<&str>| {
            target.is_some_and(|target| target_reaches_port(target, dux_port))
        };
        // The handler for this port: every foreground session's, else the
        // top level's.
        let handler_of = |config: &serde_json::Map<String, serde_json::Value>| {
            config
                .get("TCP")
                .and_then(|tcp| tcp.get(port.to_string()))
                .cloned()
        };
        let web_of = |config: &serde_json::Map<String, serde_json::Value>| {
            config
                .get("Web")
                .and_then(|web| web.as_object())
                .and_then(|web| {
                    web.iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case(target))
                        .map(|(_, handlers)| handlers.clone())
                })
        };
        let preferred = |of: &dyn Fn(
            &serde_json::Map<String, serde_json::Value>,
        ) -> Option<serde_json::Value>| {
            let in_foreground: Vec<serde_json::Value> =
                foreground.iter().filter_map(|config| of(config)).collect();
            if in_foreground.is_empty() {
                of(top).into_iter().collect()
            } else {
                in_foreground
            }
        };
        let tcp_to_dux = preferred(&handler_of)
            .iter()
            .any(|handler| names_dux(handler.get("TCPForward").and_then(|t| t.as_str())));
        let web_to_dux = preferred(&web_of).iter().any(|web| {
            web.get("Handlers")
                .and_then(|handlers| handlers.as_object())
                .is_some_and(|handlers| {
                    handlers
                        .values()
                        .any(|handler| names_dux(handler.get("Proxy").and_then(|p| p.as_str())))
                })
        });
        tcp_to_dux || web_to_dux
    }))
}

/// Whether any `TCPForward` or `Proxy` anywhere in the document names
/// `dux_port`.
fn any_target_to_port(value: &serde_json::Value, dux_port: u16) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, inner)| {
            let names_dux = (key == "TCPForward" || key == "Proxy")
                && inner.as_str().is_some_and(|target| {
                    // A port that cannot be read counts as dux's: conservative.
                    target_reaches_port(target, dux_port)
                });
            names_dux || any_target_to_port(inner, dux_port)
        }),
        serde_json::Value::Array(items) => {
            items.iter().any(|item| any_target_to_port(item, dux_port))
        }
        _ => false,
    }
}

/// Whether a forward or proxy target may reach `dux_port`: a TCP target on
/// that port, or one whose port cannot be read (conservative). A `unix:`
/// target is never dux, which listens on TCP only.
fn target_reaches_port(target: &str, dux_port: u16) -> bool {
    if target
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("unix:"))
    {
        return false;
    }
    target_port(target).is_none_or(|port| port == dux_port)
}

/// The port a forward or proxy target names: `host:port`, `scheme://host:port`
/// with any path, query or fragment after it, a bare port, or, with no port at
/// all, the scheme's default (`http` 80, `https` 443). `None` when the port
/// cannot be read (a service name, say), which the Funnel check counts as dux:
/// it cannot prove the target is anything else.
fn target_port(target: &str) -> Option<u16> {
    let (scheme, rest) = match target.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, target),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
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
    let tailscale_ips = root
        .get("Self")
        .and_then(|s| s.get("TailscaleIPs"))
        .or_else(|| root.get("TailscaleIPs"))
        .and_then(|v| v.as_array())
        .map(|ips| {
            ips.iter()
                .filter_map(|ip| ip.as_str())
                .filter_map(|ip| ip.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    Some(SelfStatus {
        dns_name,
        magic_dns_enabled,
        magic_dns_suffix,
        cert_domains,
        tailscale_ips,
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
            // "text file busy", which the probe reports as a failure.
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
mod cost_measurement {
    use super::*;

    fn stats(label: &str, mut samples: Vec<std::time::Duration>) {
        samples.sort();
        let median = samples[samples.len() / 2];
        let worst = *samples.last().unwrap();
        eprintln!(
            "{label}: median {median:?}, worst {worst:?} over {} runs",
            samples.len()
        );
    }

    fn time(runs: usize, mut f: impl FnMut()) -> Vec<std::time::Duration> {
        (0..runs)
            .map(|_| {
                let started = std::time::Instant::now();
                f();
                started.elapsed()
            })
            .collect()
    }

    /// What one watch period costs on a machine with no Tailscale, piece by
    /// piece. A measurement, not a check: run it by hand with `--ignored
    /// --nocapture`.
    #[test]
    #[ignore = "measurement"]
    fn measure_one_period_without_tailscale() {
        let missing = [
            "dux-no-such-tailscale-cost",
            "/nonexistent/dux/bin/tailscale",
            "/nonexistent/dux/Tailscale.app/Contents/MacOS/Tailscale",
        ];
        stats(
            "CLI candidates",
            time(30, || {
                let _ = first_cli_answer(&missing, |program| {
                    detect_identity_with(program, DETECT_TIMEOUT, 3890)
                });
            }),
        );
        stats("interfaces", time(30, || drop(interfaces())));
        stats(
            "sockets",
            time(30, || {
                let _ = tailscaled_detectable(&socket_answers, None, false);
            }),
        );
        stats(
            "process scan",
            time(30, || {
                let _ = tailscaled_process_running();
            }),
        );
        // A whole period's evidence on a machine where nothing of Tailscale is
        // found: the real scan runs, but reports nothing, as it would there.
        // Twelve periods of five seconds: one scan, then eleven reuses.
        let cache = std::cell::RefCell::new(ProcessScanCache::new());
        let start = std::time::Instant::now();
        let mut period = 0u64;
        stats(
            "evidence, one period (12 periods: 1 scan + 11 reused)",
            time(12, || {
                let now = start + std::time::Duration::from_secs(5 * period);
                period += 1;
                // The interfaces and sockets are read for their cost; what
                // this machine has of Tailscale is set aside, as it would be
                // absent there.
                drop(interfaces());
                let _ = evidence_from(
                    false,
                    &|path| {
                        let _ = socket_answers(path);
                        false
                    },
                    None,
                    &|| {
                        cache.borrow_mut().running(now, || {
                            let _ = tailscaled_process_running();
                            false
                        })
                    },
                );
            }),
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
        // Tailscale pairs a Funnel with the handler for its OWN host:port, so a
        // Funnel on 8443 with dux served on 443 publishes nothing of dux.
        assert_eq!(
            parse_funnel_to_port(other_port_funnelled, 3890),
            Some(false)
        );
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

    /// The exact words `tailscale status` prints (cmd/tailscale/cli/diag.go,
    /// `fixTailscaledConnectErrorImpl`) when it found no `tailscaled` (or macOS
    /// `IPNExtension`) process AND no socket: a daemon that is not running.
    const STOPPED: &[&str] = &[
        "failed to connect to local tailscaled; it doesn't appear to be running (sudo systemctl start tailscaled ?)",
        "failed to connect to local tailscaled; it doesn't appear to be running",
        "failed to connect to local Tailscale service; is Tailscale running?",
        "failed to connect to local tailscaled process; it doesn't appear to be running",
    ];

    /// What it prints when the daemon may well be there: a socket it cannot
    /// use, a process it found, or no way to look.
    const NOT_STOPPED: &[&str] = &[
        "failed to connect to local tailscaled (no tailscaled process found): dial unix /var/run/tailscale/tailscaled.sock: connect: permission denied",
        "failed to connect to local tailscaled (which appears to be running as /usr/sbin/tailscaled, pid 412). Got error: dial unix /var/run/tailscale/tailscaled.sock: connect: connection refused",
        "failed to connect to local Tailscaled process and failed to enumerate processes while looking for it",
        "Failed to connect to local Tailscale daemon for /localapi/v0/status; not running? Error: dial unix /var/run/tailscale/tailscaled.sock: connect: no such file or directory",
    ];

    #[test]
    fn only_the_clis_own_stopped_wordings_mean_a_stopped_daemon() {
        for text in STOPPED {
            assert!(daemon_stopped_message(text), "{text}");
            assert!(
                daemon_stopped_message(&format!("{text}\n")),
                "with a newline: {text}"
            );
        }
        for text in NOT_STOPPED {
            assert!(!daemon_stopped_message(text), "{text}");
        }
    }

    fn stopped_cli(name: &str, text: &str) -> super::tests::StandIn {
        super::tests::StandIn::new(
            name,
            &format!("if [ \"$1\" = \"status\" ]; then\ncat >&2 <<'EOF'\n{text}\nEOF\nexit 1\nfi"),
        )
    }

    fn nothing_here() -> LocalEvidence {
        LocalEvidence::default()
    }

    #[test]
    fn a_stopped_daemon_is_an_answer_unless_tailscale_still_owns_an_address() {
        for (n, text) in STOPPED.iter().enumerate() {
            let cli = stopped_cli(&format!("stopped-{n}"), text);
            assert_eq!(
                detect_identity_checked(&[cli.program()], DETECT_TIMEOUT, 3890, &nothing_here),
                Err(TailscaleUnavailable::DaemonStopped),
                "{text}"
            );
            assert_eq!(
                detect_identity_checked(&[cli.program()], DETECT_TIMEOUT, 3890, &|| {
                    LocalEvidence {
                        owned_address: true,
                        daemon_detectable: false,
                    }
                }),
                Err(TailscaleUnavailable::DaemonUnreachable),
                "a Tailscale-owned address says a daemon is up after all, one the CLI \
                 cannot reach: {text}"
            );
        }
        for (n, text) in NOT_STOPPED.iter().enumerate() {
            let cli = stopped_cli(&format!("not-stopped-{n}"), text);
            let got =
                detect_identity_checked(&[cli.program()], DETECT_TIMEOUT, 3890, &nothing_here);
            assert!(
                matches!(
                    got,
                    Err(TailscaleUnavailable::DaemonUnreachable
                        | TailscaleUnavailable::CommandFailed)
                ),
                "not proof of a stopped daemon, so not an answer: {text} gave {got:?}"
            );
        }
    }

    #[test]
    fn a_cli_that_is_there_but_cannot_run_is_a_failure_not_a_missing_cli() {
        let dir = crate::test_scratch::ScratchDir::new();
        let plain = dir.path().join("tailscale");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        let plain = plain.to_str().unwrap();
        assert_eq!(
            detect_identity_checked(
                &[plain, "dux-no-such-tailscale-9f1e"],
                DETECT_TIMEOUT,
                3890,
                &nothing_here
            ),
            Err(TailscaleUnavailable::CommandFailed),
            "EACCES is not proof that Tailscale is absent, and it ends the search"
        );
    }

    fn cli_with(name: &str, status: &str, serve: &str) -> super::tests::StandIn {
        super::tests::StandIn::new(
            name,
            &format!(
                "if [ \"$1\" = \"status\" ]; then printf '%s' '{status}'; \
                 else printf '%s' '{serve}'; fi"
            ),
        )
    }

    /// A stale Funnel to dux left in the serve config.
    const STALE_FUNNEL: &str = r#"{"TCP": {"443": {"TCPForward": "127.0.0.1:3890"}}, "AllowFunnel": {"box.tail.ts.net:443": true}}"#;

    #[test]
    fn a_node_that_is_down_keeps_a_saved_funnel_to_dux_and_nothing_else() {
        // A saved Funnel to dux is live again the moment `tailscale up` runs,
        // so a node that is down keeps that lockout. Anything else it saved
        // publishes nothing while it is down: no name to withdraw, no route to
        // show.
        let elsewhere = STALE_FUNNEL.replace("127.0.0.1:3890", "127.0.0.1:8080");
        for state in ["Stopped", "NeedsLogin", "NeedsMachineAuth"] {
            let status = format!(
                r#"{{"BackendState": "{state}", "Self": {{"DNSName": "box.tail.ts.net."}}}}"#
            );
            for (n, (serve, to_dux)) in [
                (STALE_FUNNEL, true),
                (elsewhere.as_str(), false),
                ("{}", false),
            ]
            .into_iter()
            .enumerate()
            {
                let cli = cli_with(&format!("down-{state}-{n}"), &status, serve);
                let look = detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890)
                    .unwrap_or_else(|err| panic!("{state}: {err:?}"));
                assert_eq!(look.funnel_to_dux, to_dux, "{state} {serve}");
                assert!(look.node_down, "{state} {serve}");
                assert!(!look.funnel, "{state} {serve}");
                assert!(look.serve.is_empty(), "{state} {serve}");
            }
        }
        // Starting, or a daemon that has not settled, may be serving any moment:
        // the serve config still decides.
        for state in ["Starting", "NoState", "Running"] {
            let status = format!(
                r#"{{"BackendState": "{state}", "Self": {{"DNSName": "box.tail.ts.net."}}}}"#
            );
            let cli = cli_with(&format!("up-{state}"), &status, STALE_FUNNEL);
            let look = detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890).unwrap();
            assert!(look.funnel_to_dux, "{state}");
            assert!(!look.node_down, "{state}");
        }
    }

    #[test]
    fn a_serve_config_printed_as_null_is_an_empty_one() {
        let cli = cli_with("null-serve", STATUS, "null");
        let look = detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890)
            .expect("`serve status --json` prints null for no config at all");
        assert!(!look.funnel && !look.funnel_to_dux && look.serve.is_empty());
    }

    #[test]
    fn the_macos_network_extensions_count_as_a_running_daemon() {
        // version/prop.go: the Standalone (macsys) and App Store extensions'
        // bundle ids, which are their executables' names.
        for name in [
            "tailscaled",
            "IPNExtension",
            "io.tailscale.ipn.macsys.network-extension",
            "io.tailscale.ipn.macos.network-extension",
            // Process tables cut names short: Linux at 15 bytes, macOS at 16.
            "io.tailscale.ip",
            "io.tailscale.ipn",
        ] {
            assert!(is_tailscale_process_name(name), "{name}");
        }
        for name in ["tailscale", "Tailscale", "io.tailscale", "sshd", ""] {
            assert!(!is_tailscale_process_name(name), "{name}");
        }
    }

    #[test]
    fn a_cli_that_is_on_no_path_is_missing_without_being_run() {
        let dir = crate::test_scratch::ScratchDir::new();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = std::ffi::OsString::from(bin.as_os_str());
        assert!(!cli_on_disk("tailscale", Some(&path)));
        assert!(!cli_on_disk("/nonexistent/dux/tailscale", Some(&path)));
        std::fs::write(bin.join("tailscale"), "").unwrap();
        assert!(cli_on_disk("tailscale", Some(&path)));
        assert!(cli_on_disk(bin.join("tailscale").to_str().unwrap(), None));
        // No PATH at all: dux cannot tell, so it asks the system by running it.
        assert!(cli_on_disk("tailscale", None));
    }

    #[test]
    fn a_process_table_that_showed_no_daemon_is_read_again_only_after_a_minute() {
        let mut cache = ProcessScanCache::default();
        let start = std::time::Instant::now();
        let scans = std::cell::Cell::new(0);
        let scan = |found: bool| {
            let scans = &scans;
            move || {
                scans.set(scans.get() + 1);
                found
            }
        };
        assert!(!cache.running(start, scan(false)));
        assert!(!cache.running(start + std::time::Duration::from_secs(5), scan(true)));
        assert_eq!(scans.get(), 1, "a recent empty scan is reused");
        assert!(cache.running(start + PROCESS_RESCAN, scan(true)));
        assert_eq!(scans.get(), 2, "read again once the minute is up");
        // A daemon seen is read again every time, so its going is noticed.
        assert!(!cache.running(start + PROCESS_RESCAN, scan(false)));
        assert_eq!(scans.get(), 3);
        assert_eq!(PROCESS_RESCAN, std::time::Duration::from_secs(60));
    }

    #[test]
    fn the_process_table_is_not_read_when_an_address_or_socket_already_answers() {
        let scanned = std::cell::Cell::new(false);
        let scan = || {
            scanned.set(true);
            false
        };
        let evidence = evidence_from(true, &|_| false, None, &scan);
        assert!(evidence.owned_address && !scanned.get());
        let evidence = evidence_from(false, &|_| true, None, &scan);
        assert!(evidence.daemon_detectable && !scanned.get());
        let evidence = evidence_from(false, &|_| false, None, &scan);
        assert_eq!(evidence, LocalEvidence::default());
        assert!(scanned.get());
    }

    #[test]
    fn every_call_asks_the_macos_app_to_run_as_the_cli() {
        // Tailscale's macOS CLI docs: TAILSCALE_BE_CLI=1 forces CLI operation,
        // so the bundled executable never opens or focuses the app.
        let cli = super::tests::StandIn::new(
            "be-cli",
            &format!(
                "if [ \"$TAILSCALE_BE_CLI\" != 1 ]; then exit 9; fi; \
                 if [ \"$1\" = \"status\" ]; then printf '%s' '{STATUS}'; \
                 else printf '%s' '{SERVE}'; fi"
            ),
        );
        assert!(detect_identity_with(cli.program(), DETECT_TIMEOUT, 3890).is_ok());
    }
    #[test]
    fn a_funnel_and_a_forward_to_dux_lock_out_wherever_each_one_sits() {
        // Tailscale pairs an AllowFunnel from the top level or any foreground
        // session with the handler for the SAME host:port from any of them, so
        // the two halves can live apart. `--tcp 443 off` leaves the top-level
        // AllowFunnel behind.
        let shapes = [
            // Top-level AllowFunnel, foreground TCP forward to dux.
            "{\"AllowFunnel\": {\"h:443\": true}, \"Foreground\": {\"s\": \
             {\"TCP\": {\"443\": {\"TCPForward\": \"localhost:3890\"}}}}}",
            // Foreground AllowFunnel, top-level web proxy to dux.
            "{\"Web\": {\"h:443\": {\"Handlers\": {\"/\": {\"Proxy\": \"http://127.0.0.1:3890\"}}}}, \
             \"Foreground\": {\"s\": {\"AllowFunnel\": {\"h:443\": true}}}}",
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

    /// The review's case, measured from a real setup's shape: a blog published
    /// through Funnel on 443, and dux served to the tailnet only on 8443.
    /// Funnel traffic reaches the 443 handler alone, so dux serves.
    #[test]
    fn a_funnel_on_one_port_does_not_pair_with_dux_served_on_another() {
        let blog_and_dux = r#"{
            "TCP": {"443": {"HTTPS": true}, "8443": {"HTTPS": true}},
            "Web": {
                "demo-box.example-tailnet.ts.net:443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:8080"}}},
                "demo-box.example-tailnet.ts.net:8443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:3890"}}}
            },
            "AllowFunnel": {"demo-box.example-tailnet.ts.net:443": true}
        }"#;
        assert_eq!(parse_funnel_to_port(blog_and_dux, 3890), Some(false));
        assert_eq!(
            parse_serve_funnel(blog_and_dux),
            Some(true),
            "the name is still withdrawn"
        );
        // Funnel on 8443 instead: that one reaches dux.
        let dux_funnelled = blog_and_dux.replace(
            "\"AllowFunnel\": {\"demo-box.example-tailnet.ts.net:443\"",
            "\"AllowFunnel\": {\"demo-box.example-tailnet.ts.net:8443\"",
        );
        assert_eq!(parse_funnel_to_port(&dux_funnelled, 3890), Some(true));
        // A raw TCP forward on another port is not paired either.
        let tcp_elsewhere = "{\"AllowFunnel\": {\"h:443\": true}, \"TCP\": {\"443\": \
             {\"TCPForward\": \"127.0.0.1:8080\"}, \"8443\": {\"TCPForward\": \"127.0.0.1:3890\"}}}";
        assert_eq!(parse_funnel_to_port(tcp_elsewhere, 3890), Some(false));
    }

    /// For one host:port Tailscale uses a foreground session's handler before
    /// the top level's (`FindTCP`, `FindWeb`).
    #[test]
    fn a_foreground_handler_for_the_funnelled_port_is_the_one_that_counts() {
        let fg_dux = "{\"AllowFunnel\": {\"h:443\": true}, \
             \"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:8080\"}}, \
             \"Foreground\": {\"s\": {\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:3890\"}}}}}";
        assert_eq!(parse_funnel_to_port(fg_dux, 3890), Some(true));
        let fg_elsewhere = "{\"AllowFunnel\": {\"h:443\": true}, \
             \"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:3890\"}}, \
             \"Foreground\": {\"s\": {\"TCP\": {\"443\": {\"TCPForward\": \"127.0.0.1:8080\"}}}}}";
        assert_eq!(parse_funnel_to_port(fg_elsewhere, 3890), Some(false));
        // An AllowFunnel key whose port cannot be read pairs with everything.
        let odd_key = "{\"AllowFunnel\": {\"h\": true}, \
             \"TCP\": {\"8443\": {\"TCPForward\": \"127.0.0.1:3890\"}}}";
        assert_eq!(parse_funnel_to_port(odd_key, 3890), Some(true));
    }

    /// dux never listens on a Unix socket, so a Funnel publishing another app
    /// through one is not dux, while a TCP port dux cannot read still counts.
    #[test]
    fn a_unix_socket_target_is_never_dux() {
        let tcp = |target: &str| {
            format!(
                "{{\"AllowFunnel\": {{\"h:443\": true}}, \"TCP\": {{\"443\": \
                 {{\"TCPForward\": \"{target}\"}}}}}}"
            )
        };
        let web = |proxy: &str| {
            format!(
                "{{\"AllowFunnel\": {{\"h:443\": true}}, \"Web\": {{\"h:443\": {{\"Handlers\": \
                 {{\"/\": {{\"Proxy\": \"{proxy}\"}}}}}}}}}}"
            )
        };
        for target in ["unix:/run/app.sock", "unix:///run/app.sock"] {
            assert_eq!(
                parse_funnel_to_port(&tcp(target), 3890),
                Some(false),
                "{target}"
            );
            assert_eq!(
                parse_funnel_to_port(&web(target), 3890),
                Some(false),
                "{target}"
            );
        }
        assert_eq!(
            parse_funnel_to_port(&tcp("localhost:dux"), 3890),
            Some(true)
        );
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
            &|| -> LocalEvidence { panic!("a CLI was found, so nothing else is asked") },
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
    fn no_cli_means_no_funnel_only_when_nothing_of_tailscale_is_here() {
        let missing = ["dux-no-such-tailscale-3c4d"];
        assert_eq!(
            detect_identity_checked(&missing, DETECT_TIMEOUT, 3890, &nothing_here),
            Err(TailscaleUnavailable::CommandMissing)
        );
        for evidence in [
            LocalEvidence {
                owned_address: true,
                daemon_detectable: false,
            },
            // Userspace networking: no interface at all, but a daemon is there.
            LocalEvidence {
                owned_address: false,
                daemon_detectable: true,
            },
        ] {
            assert_eq!(
                detect_identity_checked(&missing, DETECT_TIMEOUT, 3890, &|| evidence),
                Err(TailscaleUnavailable::Unverifiable),
                "{evidence:?}"
            );
        }
    }
    fn iface(name: &str, ip: &str) -> (String, IpAddr) {
        (name.to_string(), ip.parse().unwrap())
    }

    #[test]
    fn only_an_address_on_tailscales_own_interface_counts_not_another_vpns() {
        let known: Vec<IpAddr> = vec!["100.101.102.103".parse().unwrap()];
        // Tailscale's own interface on Linux.
        assert!(owned_tailscale_address(
            &[iface("tailscale0", "100.101.102.103")],
            &[]
        ));
        assert!(owned_tailscale_address(
            &[iface("tailscale0", "fd7a:115c:a1e0::1")],
            &[]
        ));
        // macOS (or a custom tun name): a utun whose address Tailscale reported.
        assert!(owned_tailscale_address(
            &[iface("utun4", "100.101.102.103")],
            &known
        ));
        // Other VPNs in the same CGNAT block: Cloudflare WARP (100.96.0.0/12)
        // and NetBird.
        assert!(!owned_tailscale_address(
            &[
                iface("CloudflareWARP", "100.96.0.2"),
                iface("utun3", "100.96.0.2")
            ],
            &known
        ));
        assert!(!owned_tailscale_address(
            &[iface("wt0", "100.64.0.5")],
            &known
        ));
        // A tailscale-named interface with an address outside Tailscale's ranges.
        assert!(!owned_tailscale_address(
            &[iface("tailscale0", "192.168.1.5")],
            &[]
        ));
        assert!(!owned_tailscale_address(&[], &known));
    }

    /// A socket file a crashed or removed daemon left behind is not a daemon:
    /// only one something answers on counts, or a leftover would keep dux
    /// refusing forever.
    #[test]
    fn only_a_socket_that_answers_counts_as_a_daemon() {
        let dir = crate::test_scratch::ScratchDir::new();
        let live = dir.path().join("live.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&live).unwrap();
        assert!(socket_answers(&live), "a listening daemon");

        let stale = dir.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert!(stale.exists(), "the file outlives its listener");
        assert!(!socket_answers(&stale), "a leftover socket file");

        let plain = dir.path().join("plain");
        std::fs::write(&plain, "").unwrap();
        assert!(!socket_answers(&plain), "not a socket at all");
        assert!(!socket_answers(&dir.path().join("missing.sock")));

        let started = std::time::Instant::now();
        let _ = socket_answers(&stale);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "bounded"
        );
    }

    /// A daemon too busy to take another connection is still a daemon: with
    /// its accept queue full, a non-blocking connect is refused for now
    /// (`EAGAIN`), not for good.
    #[test]
    fn a_daemon_whose_accept_queue_is_full_still_counts() {
        let dir = crate::test_scratch::ScratchDir::new();
        let path = dir.path().join("busy.sock");
        let listener =
            socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
        listener
            .bind(&socket2::SockAddr::unix(&path).unwrap())
            .unwrap();
        listener.listen(0).unwrap();
        // Fill the queue: nobody accepts, so connections pile up until the
        // next one would have to wait.
        let mut held = Vec::new();
        loop {
            let client =
                socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
            client.set_nonblocking(true).unwrap();
            match client.connect(&socket2::SockAddr::unix(&path).unwrap()) {
                Ok(()) => held.push(client),
                Err(_) => break,
            }
            assert!(held.len() < 1024, "the queue never filled");
        }
        assert!(
            socket_answers(&path),
            "a full queue is a busy daemon, not none"
        );
    }

    #[test]
    fn a_daemon_is_detectable_by_its_socket_or_its_process() {
        let none = |_: &std::path::Path| false;
        assert!(!tailscaled_detectable(&none, None, false));
        assert!(
            tailscaled_detectable(&none, None, true),
            "a running tailscaled"
        );
        for path in SOCKET_PATHS {
            let at = |p: &std::path::Path| p == std::path::Path::new(path);
            assert!(tailscaled_detectable(&at, None, false), "{path}");
        }
        let custom = |p: &std::path::Path| p == std::path::Path::new("/srv/ts/sock");
        assert!(
            tailscaled_detectable(&custom, Some("/srv/ts/sock".into()), false),
            "$TS_SOCKET"
        );
        assert!(SOCKET_PATHS.contains(&"/var/run/tailscale/tailscaled.sock"));
        assert!(SOCKET_PATHS.contains(&"/run/tailscale/tailscaled.sock"));
        assert!(
            SOCKET_PATHS.contains(&"/var/run/tailscaled.socket"),
            "macOS"
        );
        assert!(
            SOCKET_PATHS.contains(&"/tmp/tailscaled.sock"),
            "the official image"
        );
        assert!(PROCESS_NAMES.contains(&"tailscaled"));
        assert!(PROCESS_NAMES.contains(&"IPNExtension"), "macOS");
    }

    #[test]
    fn a_successful_look_remembers_this_machines_tailscale_addresses() {
        let status = parse_status_json(STATUS).unwrap();
        assert_eq!(
            status.tailscale_ips,
            vec![
                "100.101.102.103".parse::<IpAddr>().unwrap(),
                "fd7a:115c:a1e0::1234:5678".parse().unwrap()
            ]
        );
    }
    #[test]
    fn this_machines_interfaces_can_be_read_with_their_names() {
        let ifaces = interfaces();
        assert!(
            ifaces
                .iter()
                .any(|(name, ip)| ip.is_loopback() && !name.is_empty()),
            "every machine dux runs on has a named loopback interface: {ifaces:?}"
        );
    }

    #[test]
    fn a_target_with_a_path_query_or_a_named_port_is_read_conservatively() {
        let at = |proxy: &str| {
            format!(
                "{{\"AllowFunnel\": {{\"h:443\": true}}, \"Web\": {{\"h:443\": {{\"Handlers\": \
                 {{\"/\": {{\"Proxy\": \"{proxy}\"}}}}}}}}}}"
            )
        };
        for proxy in [
            "http://127.0.0.1:3890/sub?x=1",
            "http://127.0.0.1:3890?x=1",
            "http://127.0.0.1:3890#frag",
            "127.0.0.1:3890?x",
            "tcp://127.0.0.1:3890",
            // A port written by name cannot be checked here, so it counts.
            "http://127.0.0.1:http-alt",
            "localhost:dux",
        ] {
            assert_eq!(
                parse_funnel_to_port(&at(proxy), 3890),
                Some(true),
                "{proxy}"
            );
        }
        assert_eq!(
            parse_funnel_to_port(&at("http://127.0.0.1:22/x?y"), 3890),
            Some(false)
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
            tailscale_ips: Vec::new(),
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
