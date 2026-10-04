//! Address admission: which clients dux refuses before anything else, and how
//! failed password checks are counted. Its own layer, separate from sessions:
//! `blocked_addresses` applies with or without a password, to every route.
//!
//! Every knob is read from the live `[server.auth]` section on each call, so a
//! reload applies at once, and each behaves exactly as its comment in the
//! canonical config says:
//!
//! - `failed_login_window_seconds`: an address's count starts over once this
//!   long has passed since its last failure.
//! - `failed_login_delay_seconds` / `failed_login_max_delay_seconds`: after a
//!   failure, that address waits before its next attempt is checked; the wait
//!   doubles with each further failure in the window, up to the maximum. This
//!   machine waits too. 0 turns the wait off.
//! - `max_failed_logins_per_minute`: failures from all addresses together in
//!   the current minute; past it, every address but this machine is told
//!   "too many requests" until the minute is over. 0 turns it off.
//! - `max_tracked_addresses`: how many addresses are remembered at once; past
//!   it, the one whose last failure is oldest is forgotten.
//! - `max_failed_logins`: failures within the window before the address is
//!   blocked (the caller appends it to `blocked_addresses`). 0 never blocks.
//!
//! This machine (a verified loopback client) is never blocked, only slowed. A
//! loopback address is never blocked either, even when dux cannot vouch that
//! the request came from this machine (a forward onto loopback): blocking it
//! would lock the owner out with whoever was relayed.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use dux_core::config::{AddressBlock, ServerAuthConfig};

use super::provenance::Classification;

/// What failures are counted against: the client's address, or one shared
/// bucket for forwarded requests that named no readable address.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum TrackKey {
    Addr(IpAddr),
    Unknown,
}

impl TrackKey {
    fn of(classification: &Classification) -> Self {
        classification
            .client_ip
            .map(dux_core::config_auth::canonical)
            .map_or(Self::Unknown, Self::Addr)
    }
}

#[derive(Clone, Copy, Debug)]
struct Failures {
    count: u32,
    last: Instant,
    next_allowed: Instant,
}

#[derive(Default)]
struct Inner {
    tracked: HashMap<TrackKey, Failures>,
    /// The current minute of the global limit: when it began and how many
    /// failures it has seen.
    minute: Option<(Instant, u32)>,
    /// Bans that hold for this run only: their write to `config.toml` failed,
    /// or `blocked_addresses` was already at `max_blocked_addresses`.
    runtime_bans: HashSet<IpAddr>,
}

/// The admission state of one serve.
#[derive(Default)]
pub(crate) struct Admission {
    inner: std::sync::Mutex<Inner>,
}

/// Whether an address may be blocked at all.
pub(crate) fn blockable(ip: IpAddr) -> bool {
    let ip = dux_core::config_auth::canonical(ip);
    !ip.is_loopback() && !ip.is_unspecified()
}

fn ceil_secs(wait: Duration) -> u64 {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    secs.max(1)
}

impl Admission {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether this client is refused outright: its address is inside a
    /// configured `blocked_addresses` entry (`blocks`, parsed once per config)
    /// or a ban held for this run. This machine never is.
    pub(crate) fn is_blocked(&self, blocks: &[AddressBlock], c: &Classification) -> bool {
        if c.verified_this_machine() {
            return false;
        }
        let Some(ip) = c.client_ip.map(dux_core::config_auth::canonical) else {
            return false;
        };
        if blocks.iter().any(|block| block.contains(ip)) {
            return true;
        }
        self.lock().runtime_bans.contains(&ip)
    }

    /// Whether a password check may run for this client now. `Err` is how long
    /// it must wait first, in whole seconds (at least one).
    pub(crate) fn check_attempt(
        &self,
        cfg: &ServerAuthConfig,
        c: &Classification,
        now: Instant,
    ) -> Result<(), u64> {
        let window = Duration::from_secs(u64::from(cfg.failed_login_window_seconds));
        let mut inner = self.lock();
        if let Some(failures) = inner.tracked.get(&TrackKey::of(c))
            && now.saturating_duration_since(failures.last) <= window
            && now < failures.next_allowed
        {
            return Err(ceil_secs(failures.next_allowed - now));
        }
        if cfg.max_failed_logins_per_minute > 0
            && !c.verified_this_machine()
            && let Some((start, count)) = inner.minute
        {
            let elapsed = now.saturating_duration_since(start);
            if elapsed < MINUTE && count >= cfg.max_failed_logins_per_minute {
                return Err(ceil_secs(MINUTE - elapsed));
            }
        }
        inner.prune_minute(now);
        Ok(())
    }

    /// Count a failed check. Answers the address to block when this failure
    /// reaches `max_failed_logins` and the address may be blocked; its count is
    /// then forgotten, so a lifted block starts over.
    pub(crate) fn record_failure(
        &self,
        cfg: &ServerAuthConfig,
        c: &Classification,
        now: Instant,
    ) -> Option<IpAddr> {
        let key = TrackKey::of(c);
        let window = Duration::from_secs(u64::from(cfg.failed_login_window_seconds));
        let mut inner = self.lock();
        inner.prune_minute(now);
        let minute = inner.minute.get_or_insert((now, 0));
        minute.1 = minute.1.saturating_add(1);

        if !inner.tracked.contains_key(&key) {
            let cap = cfg.max_tracked_addresses.max(1) as usize;
            while inner.tracked.len() >= cap {
                let oldest = inner
                    .tracked
                    .iter()
                    .min_by_key(|(_, failures)| failures.last)
                    .map(|(key, _)| *key);
                match oldest {
                    Some(oldest) => {
                        inner.tracked.remove(&oldest);
                    }
                    None => break,
                }
            }
        }
        let failures = inner.tracked.entry(key).or_insert(Failures {
            count: 0,
            last: now,
            next_allowed: now,
        });
        if now.saturating_duration_since(failures.last) > window {
            failures.count = 0;
        }
        failures.count = failures.count.saturating_add(1);
        failures.last = now;
        failures.next_allowed = now + delay_after(cfg, failures.count);

        let TrackKey::Addr(ip) = key else {
            return None;
        };
        if cfg.max_failed_logins > 0
            && failures.count >= cfg.max_failed_logins
            && blockable(ip)
            && !c.verified_this_machine()
        {
            inner.tracked.remove(&key);
            return Some(ip);
        }
        None
    }

    /// Forget a client's failures after a successful check.
    pub(crate) fn record_success(&self, c: &Classification) {
        self.lock().tracked.remove(&TrackKey::of(c));
    }

    /// Hold a ban for the rest of this run.
    pub(crate) fn ban_for_this_run(&self, ip: IpAddr) {
        self.lock()
            .runtime_bans
            .insert(dux_core::config_auth::canonical(ip));
    }

    /// Stop holding a ban for this run: `blocked_addresses` holds it now.
    pub(crate) fn lift_ban_for_this_run(&self, ip: IpAddr) {
        self.lock()
            .runtime_bans
            .remove(&dux_core::config_auth::canonical(ip));
    }

    /// How many addresses the failure tracker remembers right now.
    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.lock().tracked.len()
    }
}

const MINUTE: Duration = Duration::from_secs(60);

impl Inner {
    /// Start a new minute when the current one is over.
    fn prune_minute(&mut self, now: Instant) {
        if self
            .minute
            .is_some_and(|(start, _)| now.saturating_duration_since(start) >= MINUTE)
        {
            self.minute = None;
        }
    }
}

/// The wait after the `count`th failure in the window: the configured delay,
/// doubled for each failure after the first, up to the maximum.
fn delay_after(cfg: &ServerAuthConfig, count: u32) -> Duration {
    let base = u64::from(cfg.failed_login_delay_seconds);
    if base == 0 {
        return Duration::ZERO;
    }
    let doublings = count.saturating_sub(1).min(32);
    let wait = base
        .saturating_mul(1u64 << doublings)
        .min(u64::from(cfg.failed_login_max_delay_seconds).max(base));
    Duration::from_secs(wait)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::provenance::ClientClass;

    fn from(class: ClientClass, ip: &str) -> Classification {
        Classification {
            class,
            client_ip: ip.parse().ok(),
            transport_encrypted: false,
            https_serve_route: false,
            unvouched_proxy: false,
        }
    }

    fn net(ip: &str) -> Classification {
        from(ClientClass::Network, ip)
    }

    fn cfg() -> ServerAuthConfig {
        ServerAuthConfig::default()
    }

    #[test]
    fn the_wait_doubles_with_each_failure_up_to_the_maximum_and_zero_turns_it_off() {
        let c = cfg();
        let waits: Vec<u64> = (1..=7).map(|n| delay_after(&c, n).as_secs()).collect();
        assert_eq!(waits, vec![1, 2, 4, 8, 16, 30, 30]);
        let off = ServerAuthConfig {
            failed_login_delay_seconds: 0,
            ..cfg()
        };
        assert_eq!(delay_after(&off, 5), Duration::ZERO);
    }

    #[test]
    fn a_failure_makes_that_address_wait_and_only_that_address() {
        let a = Admission::default();
        let t0 = Instant::now();
        assert_eq!(a.check_attempt(&cfg(), &net("198.51.100.1"), t0), Ok(()));
        assert_eq!(a.record_failure(&cfg(), &net("198.51.100.1"), t0), None);
        assert_eq!(a.check_attempt(&cfg(), &net("198.51.100.1"), t0), Err(1));
        assert_eq!(a.check_attempt(&cfg(), &net("198.51.100.2"), t0), Ok(()));
        let later = t0 + Duration::from_millis(1001);
        assert_eq!(a.check_attempt(&cfg(), &net("198.51.100.1"), later), Ok(()));
        a.record_failure(&cfg(), &net("198.51.100.1"), later);
        assert_eq!(
            a.check_attempt(&cfg(), &net("198.51.100.1"), later),
            Err(2),
            "doubled"
        );
    }

    #[test]
    fn the_count_starts_over_after_the_window_and_on_a_success() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            failed_login_delay_seconds: 0,
            max_failed_logins: 3,
            failed_login_window_seconds: 10,
            ..cfg()
        };
        let ip = net("198.51.100.3");
        let t0 = Instant::now();
        a.record_failure(&c, &ip, t0);
        a.record_failure(&c, &ip, t0);
        // Past the window: the third failure is the first of a new count.
        let late = t0 + Duration::from_secs(11);
        assert_eq!(a.record_failure(&c, &ip, late), None);
        assert_eq!(a.record_failure(&c, &ip, late), None);
        a.record_success(&ip);
        assert_eq!(a.record_failure(&c, &ip, late), None);
        assert_eq!(a.record_failure(&c, &ip, late), None);
        assert_eq!(
            a.record_failure(&c, &ip, late),
            Some("198.51.100.3".parse().unwrap()),
            "three in the window blocks"
        );
        assert_eq!(a.tracked(), 0, "a blocked address's count is forgotten");
    }

    #[test]
    fn this_machine_and_loopback_are_slowed_but_never_blocked() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            failed_login_delay_seconds: 0,
            max_failed_logins: 1,
            ..cfg()
        };
        let t0 = Instant::now();
        let machine = from(ClientClass::ThisMachine, "127.0.0.1");
        assert_eq!(a.record_failure(&c, &machine, t0), None);
        // Loopback dux cannot vouch for (a forward onto it) is not blocked either.
        assert_eq!(a.record_failure(&c, &net("127.0.0.1"), t0), None);
        assert_eq!(a.record_failure(&c, &net("::1"), t0), None);
        assert_eq!(
            a.record_failure(&c, &net("::ffff:198.51.100.4"), t0),
            Some("198.51.100.4".parse().unwrap()),
            "a mapped address is blocked as the IPv4 address it carries"
        );
        let slowed = cfg();
        a.record_failure(&slowed, &machine, t0);
        assert!(
            a.check_attempt(&slowed, &machine, t0).is_err(),
            "this machine waits too"
        );
        // Nothing to block for a forwarded request with no readable address.
        let unknown = from(ClientClass::Network, "garbage");
        assert_eq!(a.record_failure(&c, &unknown, t0), None);
    }

    #[test]
    fn the_global_limit_stops_everyone_but_this_machine_until_the_minute_ends() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            failed_login_delay_seconds: 0,
            max_failed_logins: 0,
            max_failed_logins_per_minute: 3,
            ..cfg()
        };
        let t0 = Instant::now();
        for n in 0..3 {
            a.record_failure(&c, &net(&format!("198.51.100.{n}")), t0);
        }
        let fresh = net("203.0.113.50");
        assert_eq!(
            a.check_attempt(&c, &fresh, t0 + Duration::from_secs(10)),
            Err(50)
        );
        assert_eq!(
            a.check_attempt(&c, &from(ClientClass::ThisMachine, "127.0.0.1"), t0),
            Ok(())
        );
        assert_eq!(
            a.check_attempt(&c, &fresh, t0 + Duration::from_secs(60)),
            Ok(())
        );
        let off = ServerAuthConfig {
            max_failed_logins_per_minute: 0,
            ..c
        };
        assert_eq!(a.check_attempt(&off, &fresh, t0), Ok(()));
    }

    #[test]
    fn the_tracker_forgets_the_oldest_address_past_its_bound() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            max_tracked_addresses: 2,
            failed_login_delay_seconds: 5,
            ..cfg()
        };
        let t0 = Instant::now();
        a.record_failure(&c, &net("198.51.100.1"), t0);
        a.record_failure(&c, &net("198.51.100.2"), t0 + Duration::from_millis(1));
        a.record_failure(&c, &net("198.51.100.3"), t0 + Duration::from_millis(2));
        assert_eq!(a.tracked(), 2);
        assert_eq!(
            a.check_attempt(&c, &net("198.51.100.1"), t0 + Duration::from_millis(3)),
            Ok(()),
            "the oldest was forgotten"
        );
        assert!(a.check_attempt(&c, &net("198.51.100.3"), t0).is_err());
    }

    #[test]
    fn blocks_match_ranges_and_bans_held_for_this_run() {
        let a = Admission::default();
        let blocks = vec![AddressBlock::parse("203.0.113.0/24").unwrap()];
        assert!(a.is_blocked(&blocks, &net("203.0.113.9")));
        assert!(a.is_blocked(&blocks, &net("::ffff:203.0.113.9")));
        assert!(!a.is_blocked(&blocks, &net("198.51.100.9")));
        a.ban_for_this_run("198.51.100.9".parse().unwrap());
        assert!(a.is_blocked(&blocks, &net("198.51.100.9")));
        let everything = vec![AddressBlock::parse("0.0.0.0/0").unwrap()];
        assert!(
            !a.is_blocked(&everything, &from(ClientClass::ThisMachine, "127.0.0.1")),
            "this machine is never refused"
        );
    }
}
