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

    fn say(&self, loudness: Loudness, key: &str, message: &str, quiet_web: bool) {
        dux_core::logger::warn(&format!("[server] {message}"));
        match loudness {
            Loudness::Warning => self.console.warn(message),
            Loudness::Alarm => self.console.error(message),
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
        self.say(Loudness::Warning, PROXY_KEY, &proxy_sentence(), false);
    }

    pub(crate) fn weak_password(&self) {
        self.say(Loudness::Warning, WEAK_KEY, &weak_sentence(), false);
    }

    pub(crate) fn blocked(&self, entry: &str, failures: u32, path: &str, kept: BanKept) {
        self.say(
            Loudness::Warning,
            BLOCKED_KEY,
            &blocked_sentence(entry, failures, path, &kept),
            false,
        );
    }

    /// An address dux could not verify reached `max_failed_logins`: say so,
    /// and that nothing was written.
    pub(crate) fn unverified_limit(&self, ip: IpAddr, why: Unverified, failures: u32, path: &str) {
        self.say(
            Loudness::Warning,
            BLOCKED_KEY,
            &unverified_limit_sentence(ip, why, failures, path),
            false,
        );
    }

    /// A `blocked_addresses` entry covers loopback or an address of this
    /// machine, which the blocklist never applies to.
    pub(crate) fn own_entry(&self, entry: &str, ip: IpAddr) {
        self.say(
            Loudness::Warning,
            OWN_ENTRY_KEY,
            &own_entry_sentence(entry, ip),
            false,
        );
    }

    /// A raw TCP forward onto dux's port appeared.
    pub(crate) fn forward_known(&self) {
        self.say(Loudness::Warning, FORWARD_KEY, &forward_sentence(), false);
    }

    /// The forward is gone.
    pub(crate) fn forward_gone(&self) {
        self.withdraw(FORWARD_KEY);
    }

    fn exposed(&self, reach: &[String], funnel: bool) {
        self.say(
            Loudness::Alarm,
            EXPOSED_KEY,
            &exposed_sentence(reach, funnel),
            true,
        );
    }
}

/// The alarm for a dux reachable beyond this machine with no password.
pub(crate) fn exposed_sentence(reach: &[String], funnel: bool) -> String {
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
    format!(
        "No password is set and dux is reachable beyond this machine{how}: anyone who can reach \
         it controls your agents and terminals. Set one with `dux config set \
         server.auth.password`."
    )
}

fn proxy_sentence() -> String {
    "A request reached dux through a proxy on this machine (it carried a forwarding header such \
     as X-Forwarded-For). dux counts such requests as the network unless they come through a \
     confirmed tailscale serve route, but a proxy that forwards WITHOUT that header makes \
     outside visitors look like this machine, which needs no password under the current \
     [server.auth] require. Behind a reverse proxy, set require = \"everywhere\"."
        .to_string()
}

fn weak_sentence() -> String {
    "Someone signed in to the web UI with a password below the minimums in [server.auth] \
     (minimum_password_length and minimum_password_score). It still works, but change it with \
     `dux config set server.auth.password` or from the web UI's Preferences."
        .to_string()
}

fn blocked_sentence(entry: &str, failures: u32, path: &str, kept: &BanKept) -> String {
    let why = format!("dux blocked {entry} after {failures} failed sign-ins.");
    match kept {
        BanKept::InConfig => format!(
            "{why} It is in blocked_addresses in the [server.auth] section of {path}; if it was \
             you, remove it there and reload the config (the web UI's Reload config, \
             `kill -USR1` on dux, or any `dux config set`)."
        ),
        BanKept::ListFull { max } => format!(
            "{why} blocked_addresses in {path} already has {max} entries \
             (max_blocked_addresses), so the block was not written there: it holds until dux \
             restarts."
        ),
        BanKept::WriteFailed { error } => format!(
            "{why} Writing it to blocked_addresses in {path} failed ({error}), so it holds only \
             until dux restarts."
        ),
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

fn unverified_limit_sentence(ip: IpAddr, why: Unverified, failures: u32, path: &str) -> String {
    let ip = dux_core::config_auth::canonical(ip);
    let (came, because, trust) = match why {
        Unverified::Funnel => (
            "through a Tailscale Funnel".to_string(),
            "Funnel traffic comes from the internet, and dux cannot confirm the address it \
             names"
                .to_string(),
            "If you are sure of it",
        ),
        Unverified::ExposureUnconfirmed(cause) => (
            "through what looked like tailscale serve".to_string(),
            format!("{cause}, so it cannot vouch for that route"),
            "If you are sure of it",
        ),
        Unverified::UnconfirmedProxy => (
            "through a proxy".to_string(),
            "the proxy is not a confirmed tailscale serve route, so the client may have \
             chosen it"
                .to_string(),
            "If you trust the proxy",
        ),
    };
    format!(
        "{failures} failed sign-ins came {came} claiming to be {ip}. dux did not add it to \
         blocked_addresses because it could not verify that address ({because}); it keeps \
         slowing those sign-ins down. {trust}, add {ip} to blocked_addresses in the \
         [server.auth] section of {path} by hand and reload the config."
    )
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
fn forward_sentence() -> String {
    "A tailscale serve TCP forward reaches dux's port. Whoever comes in through it arrives \
     over plain loopback with nothing to tell them apart from this machine, so dux counts every \
     such request as the network (with a password set, someone on this machine signs in too), \
     and their failed sign-ins slow down sign-ins on this machine over loopback as well. \
     Remove the forward (`tailscale serve status` lists it) to separate them."
        .to_string()
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
