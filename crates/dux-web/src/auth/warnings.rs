//! What the login says about itself, worded once and said the same way in all
//! three serving modes. Every sentence goes to the same three places: dux.log,
//! the serve's console (the shared log-line model `dux server` prints and the
//! start-web-server flip's viewer shows; the background serve has none), and
//! the engine's keyed status, which the terminal UI's status line and the web's
//! toasts both read. The web keeps its own banner for the missing password, so
//! that one status is quiet there.
//!
//! - No password while dux is reachable beyond this machine: an alarm (the
//!   console's red tone), said when it becomes true and withdrawn when a
//!   password is set or the reach goes.
//! - The one-time proxy warning, on the first forwarded request dux cannot
//!   vouch for while `require` is not `"everywhere"`.
//! - A weak password, said once per password when it signs in below today's
//!   minimums.
//! - Every block dux adds, with where to lift it.

use std::net::IpAddr;

use dux_core::engine::StatusUpdate;
use dux_core::status_text::StatusText;
use dux_core::statusline::{QuietSurfaces, StatusTone};

use super::AuthState;

/// The keyed status the no-password alarm is posted (and withdrawn) under.
pub(crate) const EXPOSED_KEY: &str = "server-auth-no-password";
const PROXY_KEY: &str = "server-auth-proxy";
const WEAK_KEY: &str = "server-auth-weak-password";
const BLOCKED_KEY: &str = "server-auth-blocked";
const OWN_ENTRY_KEY: &str = "server-auth-own-entry";
const FORWARD_KEY: &str = "server-auth-forward";

/// Says a sentence everywhere it belongs.
pub(crate) struct Speaker {
    console: crate::console::Console,
    engine: Option<crate::engine_actor::EngineHandle>,
}

/// How loudly.
#[derive(Clone, Copy)]
enum Loudness {
    Warning,
    /// A warning painted in the console's error tone (red), for the one that
    /// means anyone can drive your terminals.
    Alarm,
}

/// Where a block is held.
pub(crate) enum BanKept {
    /// In `blocked_addresses`, where the owner lifts it.
    InConfig,
    /// In memory for this run: the list is already `max_blocked_addresses`
    /// long.
    ListFull { max: u32 },
    /// In memory for this run: writing it failed.
    WriteFailed { error: String },
}

impl Speaker {
    pub(crate) fn new(
        console: crate::console::Console,
        engine: Option<crate::engine_actor::EngineHandle>,
    ) -> Self {
        Self { console, engine }
    }

    /// The log and the console read the plain spelling; the status carries the
    /// parts too, so the web draws each command as a chip.
    fn say(&self, loudness: Loudness, key: &str, message: StatusText, quiet_web: bool) {
        dux_core::logger::warn(&format!("[server] {message}"));
        match loudness {
            Loudness::Warning => self.console.warn(&message),
            Loudness::Alarm => self.console.error(&message),
        }
        if let Some(engine) = &self.engine {
            let mut update = StatusUpdate::keyed(key, StatusTone::Warning, message);
            if quiet_web {
                update.quiet_on = QuietSurfaces::WEB;
            }
            engine.post_status(update);
        }
    }

    fn withdraw(&self, key: &str) {
        if let Some(engine) = &self.engine {
            engine.clear_status(key);
        }
    }

    pub(crate) fn proxy_warning(&self) {
        self.say(Loudness::Warning, PROXY_KEY, proxy_sentence().into(), false);
    }

    pub(crate) fn weak_password(&self) {
        self.say(Loudness::Warning, WEAK_KEY, weak_sentence(), false);
    }

    pub(crate) fn blocked(&self, entry: &str, failures: u32, path: &str, kept: BanKept) {
        self.say(
            Loudness::Warning,
            BLOCKED_KEY,
            blocked_sentence(entry, failures, path, &kept),
            false,
        );
    }

    /// An address dux could not verify reached `max_failed_logins`: say so,
    /// and that nothing was written.
    pub(crate) fn unverified_limit(&self, ip: IpAddr, why: Unverified, failures: u32, path: &str) {
        self.say(
            Loudness::Warning,
            BLOCKED_KEY,
            unverified_limit_sentence(ip, why, failures, path),
            false,
        );
    }

    /// A `blocked_addresses` entry covers loopback or an address of this
    /// machine, which the blocklist never applies to.
    pub(crate) fn own_entry(&self, entry: &str, ip: IpAddr) {
        self.say(
            Loudness::Warning,
            OWN_ENTRY_KEY,
            own_entry_sentence(entry, ip).into(),
            false,
        );
    }

    /// A raw TCP forward onto dux's port appeared.
    pub(crate) fn forward_known(&self) {
        self.say(Loudness::Warning, FORWARD_KEY, forward_sentence(), false);
    }

    /// The forward is gone.
    pub(crate) fn forward_gone(&self) {
        self.withdraw(FORWARD_KEY);
    }

    fn exposed(&self, reach: &[String], funnel: bool) {
        self.say(
            Loudness::Alarm,
            EXPOSED_KEY,
            exposed_sentence(reach, funnel),
            true,
        );
    }
}

/// The alarm for a dux reachable beyond this machine with no password.
pub(crate) fn exposed_sentence(reach: &[String], funnel: bool) -> StatusText {
    let mut ways: Vec<String> = Vec::new();
    if !reach.is_empty() {
        ways.push(format!("it listens on {}", reach.join(", ")));
    }
    if funnel {
        ways.push(
            "a Tailscale Funnel or forward reaches its port from beyond this machine".to_string(),
        );
    }
    let how = if ways.is_empty() {
        String::new()
    } else {
        format!(" ({})", ways.join("; "))
    };
    dux_core::status_text![
        format!(
            "No password is set and dux is reachable beyond this machine{how}: anyone who can \
             reach it controls your agents and terminals. Set one with "
        ),
        q(dux_core::config_auth::SET_PASSWORD_COMMAND),
        "."
    ]
}

fn proxy_sentence() -> String {
    "A request reached dux through a proxy on this machine (it carried a forwarding header such \
     as X-Forwarded-For). dux counts such requests as the network unless they come through a \
     confirmed tailscale serve route, but a proxy that forwards WITHOUT that header makes \
     outside visitors look like this machine, which needs no password under the current \
     [server.auth] require. Behind a reverse proxy, set require = \"everywhere\"."
        .to_string()
}

fn weak_sentence() -> StatusText {
    dux_core::status_text![
        "Someone signed in to the web UI with a password below the minimums in [server.auth] \
         (minimum_password_length and minimum_password_score). It still works, but change it \
         with ",
        q(dux_core::config_auth::SET_PASSWORD_COMMAND),
        " or from the web UI's Preferences."
    ]
}

fn blocked_sentence(entry: &str, failures: u32, path: &str, kept: &BanKept) -> StatusText {
    // A range is the IPv6 /64 the failures came from, written whole rather
    // than one address at a time.
    let why = if entry.contains('/') {
        format!(
            "dux blocked {entry}, the IPv6 /64 they came from, as one range after {failures} \
             failed sign-ins from addresses in it."
        )
    } else {
        format!("dux blocked {entry} after {failures} failed sign-ins.")
    };
    match kept {
        BanKept::InConfig => dux_core::status_text![
            format!(
                "{why} It is in blocked_addresses in the [server.auth] section of {path}; if it \
                 was you, remove it there and reload the config (the web UI's Reload config, "
            ),
            q("kill -USR1"),
            " on dux, or any ",
            q("dux config set"),
            ")."
        ],
        BanKept::ListFull { max } => format!(
            "{why} blocked_addresses in {path} already has {max} entries \
             (max_blocked_addresses), so the block was not written there: it holds until dux \
             restarts."
        )
        .into(),
        BanKept::WriteFailed { error } => format!(
            "{why} Writing it to blocked_addresses in {path} failed ({error}), so it holds only \
             until dux restarts."
        )
        .into(),
    }
}

/// An unverified forwarded address at the limit. Decided: it is NOT written,
/// because the address is only what the request claimed and writing it would
/// let anyone fill the blocklist with addresses of their choosing.
/// Why the address a request claimed could not be verified, which the
/// unverified-limit warning names (decided, after review: the reason it once
/// gave, "not a confirmed tailscale serve route", was wrong for Funnel
/// traffic and for a serve route dux could not yet vouch for).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unverified {
    /// It came through a Tailscale Funnel: the internet, whose address
    /// Tailscale names but dux cannot confirm.
    Funnel,
    /// It looked like `tailscale serve`, but dux could not vouch for what
    /// reaches its port: the cause, in words.
    ExposureUnconfirmed(&'static str),
    /// It came through a proxy that is not a confirmed `tailscale serve`
    /// route.
    UnconfirmedProxy,
}

impl Unverified {
    /// The reason for a request whose claimed address dux could not verify.
    pub(crate) fn of(c: &super::provenance::Classification) -> Self {
        use super::provenance::{ClientClass, Via};
        match (c.class, c.via, c.loopback_distrusted) {
            (ClientClass::Internet, _, _) => Self::Funnel,
            (_, Via::Forwarded, Some(cause)) => Self::ExposureUnconfirmed(cause),
            _ => Self::UnconfirmedProxy,
        }
    }
}

fn unverified_limit_sentence(ip: IpAddr, why: Unverified, failures: u32, path: &str) -> StatusText {
    let ip = dux_core::config_auth::canonical(ip);
    let (came, because, trust): (&str, StatusText, &str) = match why {
        Unverified::Funnel => (
            "through a Tailscale Funnel",
            "Funnel traffic comes from the internet, and dux cannot confirm the address it \
             names"
                .into(),
            "If you are sure of it",
        ),
        Unverified::ExposureUnconfirmed(cause) => (
            "through what looked like tailscale serve",
            dux_core::status_text![marked(cause), ", so it cannot vouch for that route"],
            "If you are sure of it",
        ),
        Unverified::UnconfirmedProxy => (
            "through a proxy",
            "the proxy is not a confirmed tailscale serve route, so the client may have \
             chosen it"
                .into(),
            "If you trust the proxy",
        ),
    };
    dux_core::status_text![
        format!(
            "{failures} failed sign-ins came {came} claiming to be {ip}. dux did not add it to \
             blocked_addresses because it could not verify that address ("
        ),
        because,
        format!(
            "); it keeps slowing those sign-ins down. {trust}, add {ip} to blocked_addresses \
             in the [server.auth] section of {path} by hand and reload the config."
        )
    ]
}

/// A reason the exposure gate or the provenance check gives, as prose. Those
/// reasons are dux's own constants, written with their setting and command
/// names between backticks because the sign-in page renders them through the
/// browser's inline-code chips (`required_reason`); here each such span is a
/// name, so a status draws it as a chip too and its plain spelling quotes it.
/// Only a `&'static str` is read this way, so no text from outside dux is.
fn marked(reason: &'static str) -> dux_core::prose::Prose {
    let mut prose = dux_core::prose::Prose::new();
    for (n, part) in reason.split('`').enumerate() {
        if n % 2 == 1 {
            prose.push_quoted(part);
        } else {
            prose.push_text(part);
        }
    }
    prose
}

/// The entries of `blocked_addresses` that cover loopback or one of `own`
/// (this machine's own addresses), each with the address it covers. Config
/// validation still accepts them (decided, after review): they are not wrong
/// so much as inert here, and refusing to start over one would be worse.
pub(crate) fn own_entries(entries: &[String], own: &[IpAddr]) -> Vec<(String, IpAddr)> {
    let mut mine: Vec<IpAddr> = vec![
        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
    ];
    mine.extend(own.iter().copied());
    entries
        .iter()
        .filter_map(|entry| {
            let block = dux_core::config_auth::AddressBlock::parse(entry).ok()?;
            let ip = mine.iter().find(|ip| block.contains(**ip))?;
            Some((entry.clone(), *ip))
        })
        .collect()
}

fn own_entry_sentence(entry: &str, ip: IpAddr) -> String {
    format!(
        "blocked_addresses in [server.auth] has \"{entry}\", which covers {ip}, an address of \
         this machine. dux never applies the blocklist to this machine's own addresses or to \
         loopback (tailscale serve relays every tailnet device through it), so that part of the \
         entry never refuses anyone."
    )
}

/// What dux says when a raw TCP forward onto its port appears. The shared
/// slow-down is the one limit the per-route buckets cannot lift (decided,
/// after review), so it is said here rather than left for someone to find.
fn forward_sentence() -> StatusText {
    dux_core::status_text![
        "A tailscale serve TCP forward reaches dux's port. Whoever comes in through it arrives \
         over plain loopback with nothing to tell them apart from this machine, so dux counts \
         every such request as the network (with a password set, someone on this machine signs \
         in too), and their failed sign-ins slow down sign-ins on this machine over loopback as \
         well. Remove the forward (",
        q("tailscale serve status"),
        " lists it) to separate them."
    ]
}

/// Follows whether the no-password alarm is due, saying it when it becomes
/// true and withdrawing its status when it stops being true.
#[derive(Default)]
pub(crate) struct ExposedWarning {
    said: bool,
}

impl ExposedWarning {
    pub(crate) fn check(&mut self, state: &AuthState) {
        let reach = state.reach.beyond_loopback();
        let published = state.exposure().known_published();
        let due = !state.snapshot().has_password() && (!reach.is_empty() || published);
        if due && !self.said {
            state.speaker.exposed(&reach, published);
        } else if !due && self.said {
            state.speaker.withdraw(EXPOSED_KEY);
        }
        self.said = due;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a sentence carries as chips.
    fn names(text: &StatusText) -> Vec<String> {
        text.segments()
            .expect("built from parts")
            .iter()
            .filter_map(|segment| match segment {
                dux_core::prose::ProseSegment::Name { name, .. } => Some(name.clone()),
                dux_core::prose::ProseSegment::Text(_) => None,
            })
            .collect()
    }

    /// A command reaches the web as a chip and the status line in quotes,
    /// never between backticks, which neither surface renders.
    #[test]
    fn every_command_a_warning_names_is_a_chip() {
        let set = "dux config set server.auth.password";
        for (text, commands) in [
            (
                exposed_sentence(&["192.0.2.1".to_string()], false),
                vec![set],
            ),
            (weak_sentence(), vec![set]),
            (
                blocked_sentence("203.0.113.7", 5, "/x/config.toml", &BanKept::InConfig),
                vec!["kill -USR1", "dux config set"],
            ),
            (forward_sentence(), vec!["tailscale serve status"]),
            (
                unverified_limit_sentence(
                    "203.0.113.9".parse().unwrap(),
                    Unverified::ExposureUnconfirmed("`[server] tailscale` is `no`"),
                    10,
                    "/x/config.toml",
                ),
                vec!["[server] tailscale", "no"],
            ),
        ] {
            assert!(!text.contains('`'), "{text}");
            assert_eq!(names(&text), commands, "{text}");
            for command in commands {
                assert!(text.contains(&format!("\"{command}\"")), "{text}");
            }
        }
    }

    #[test]
    fn entries_covering_loopback_or_an_own_address_are_found_and_named() {
        let entries: Vec<String> = [
            "127.0.0.0/8",
            "203.0.113.0/24",
            "::1",
            "192.0.2.0/24",
            "0.0.0.0/0",
            "bad",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        let own: Vec<IpAddr> = vec!["192.0.2.10".parse().unwrap()];
        let found: Vec<String> = own_entries(&entries, &own)
            .into_iter()
            .map(|(entry, ip)| format!("{entry} {ip}"))
            .collect();
        assert_eq!(
            found,
            [
                "127.0.0.0/8 127.0.0.1",
                "::1 ::1",
                "192.0.2.0/24 192.0.2.10",
                "0.0.0.0/0 127.0.0.1"
            ]
        );
        let text = own_entry_sentence("192.0.2.0/24", "192.0.2.10".parse().unwrap());
        assert!(
            text.contains("192.0.2.0/24") && text.contains("192.0.2.10") && text.contains("never"),
            "{text}"
        );
        let unverified = unverified_limit_sentence(
            "203.0.113.9".parse().unwrap(),
            Unverified::UnconfirmedProxy,
            10,
            "/x/config.toml",
        );
        assert!(
            !unverified.contains("  "),
            "no runs of spaces: {unverified}"
        );
    }

    #[test]
    fn the_forward_note_says_it_shares_this_machines_slow_down() {
        let text = forward_sentence();
        assert!(
            text.contains("slow down sign-ins on this machine"),
            "{text}"
        );
        assert!(!text.contains("  "), "{text}");
    }

    #[test]
    fn the_unverified_reason_follows_how_the_request_came() {
        use crate::auth::provenance::{Classification, ClientClass, Via};
        let base = Classification {
            class: ClientClass::Network,
            named: Vec::new(),
            verified_ip: None,
            claimed_ip: Some("203.0.113.9".parse().unwrap()),
            transport_encrypted: false,
            https_serve_route: false,
            unvouched_proxy: true,
            loopback_distrusted: None,
            via: Via::Forwarded,
            own_network: false,
        };
        assert_eq!(Unverified::of(&base), Unverified::UnconfirmedProxy);
        let funnel = Classification {
            class: ClientClass::Internet,
            ..base.clone()
        };
        assert_eq!(Unverified::of(&funnel), Unverified::Funnel);
        let gated = Classification {
            loopback_distrusted: Some("dux has not finished its first look at Tailscale"),
            ..base
        };
        assert_eq!(
            Unverified::of(&gated),
            Unverified::ExposureUnconfirmed("dux has not finished its first look at Tailscale")
        );
    }

    #[test]
    fn an_unverified_address_at_the_limit_says_it_was_not_written_and_how_to_add_it() {
        for (why, says) in [
            (Unverified::Funnel, "Tailscale Funnel"),
            (
                Unverified::ExposureUnconfirmed("dux has not finished its first look at Tailscale"),
                "first look at Tailscale",
            ),
            (
                Unverified::UnconfirmedProxy,
                "not a confirmed tailscale serve route",
            ),
        ] {
            let text = unverified_limit_sentence(
                "203.0.113.9".parse().unwrap(),
                why,
                10,
                "/x/config.toml",
            );
            assert!(text.contains("203.0.113.9"), "{text}");
            assert!(text.contains("did not add it"), "{text}");
            assert!(text.contains("could not verify"), "{text}");
            assert!(text.contains(says), "{why:?}: {text}");
            assert!(text.contains("by hand"), "{text}");
            assert!(text.contains("/x/config.toml"), "{text}");
            assert!(!text.contains("  "), "{text}");
            if why != Unverified::UnconfirmedProxy {
                assert!(
                    !text.contains("not a confirmed tailscale serve route"),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn the_alarm_names_the_reach_the_risk_and_the_fix() {
        let text = exposed_sentence(&["every IPv4 address".to_string()], false);
        let lower = text.to_ascii_lowercase();
        assert!(lower.contains("no password"), "{text}");
        assert!(text.contains("every IPv4 address"), "{text}");
        assert!(
            text.contains("dux config set server.auth.password"),
            "{text}"
        );
        let funnel = exposed_sentence(&[], true);
        assert!(funnel.contains("Funnel"), "{funnel}");
    }

    #[test]
    fn the_proxy_warning_suggests_everywhere_once_per_line() {
        let text = proxy_sentence();
        assert_eq!(text.matches("everywhere").count(), 1, "{text}");
        assert!(text.contains("proxy"), "{text}");
    }

    #[test]
    fn a_range_block_says_it_is_the_slash64_as_one_range() {
        let text = blocked_sentence("2001:db8:1:2::/64", 5, "/x/config.toml", &BanKept::InConfig);
        assert!(text.contains("2001:db8:1:2::/64"), "{text}");
        assert!(
            text.contains("IPv6 /64") && text.contains("one range"),
            "{text}"
        );
        assert!(!text.contains("  "), "{text}");
    }

    #[test]
    fn a_block_says_where_it_lives_and_how_long_it_holds() {
        let ip = "203.0.113.7";
        let kept = blocked_sentence(
            ip,
            5,
            "/home/me/.config/dux/config.toml",
            &BanKept::InConfig,
        );
        assert!(
            kept.contains("203.0.113.7") && !kept.contains("::ffff"),
            "{kept}"
        );
        assert!(
            kept.contains("blocked_addresses") && kept.contains("config.toml"),
            "{kept}"
        );
        let full = blocked_sentence(ip, 5, "c", &BanKept::ListFull { max: 3 });
        assert!(full.contains("until dux restarts") && full.contains("max_blocked_addresses"));
        let failed = blocked_sentence(
            ip,
            5,
            "c",
            &BanKept::WriteFailed {
                error: "disk full".into(),
            },
        );
        assert!(failed.contains("disk full") && failed.contains("until dux restarts"));
    }
}
