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
//!   machine waits too, and so does a wrong CURRENT password on the
//!   password-change route, which is accounted exactly like a failed login
//!   (decided, after review: exempting it left the tailnet, which has no
//!   global limit, free to guess as fast as dux hashes). 0 turns the wait off.
//! - `max_failed_logins_per_minute`: failures from many addresses together in
//!   the current minute, counted apart for each trust level (see [`Level`])
//!   and, for unverified requests, for each route they took (see [`Via`]);
//!   past it, that level is told "too many requests" until the minute is
//!   over. Tailnet devices and this machine have no such limit, only their
//!   own per-address wait. 0 turns it off.
//! - `max_tracked_addresses`: how many addresses are remembered at once; past
//!   it, the one whose last failure is oldest is forgotten.
//! - `max_failed_logins`: failures within the window before the address is
//!   blocked (the caller appends it to `blocked_addresses`). 0 never blocks.
//!
//! A check is reserved when it is admitted ([`Reservation`]) and counts as a
//! failure that may yet land until it is settled, so attempts sent at the
//! same moment from one address cannot all skip the wait between them.
//!
//! WHICH address a failure counts against is decided by what dux can verify
//! (decided, after review). A verified address (the direct peer on a listener
//! that is not loopback, or the client of a request proven to come through
//! `tailscale serve`) is counted and, at the limit, blocked. Every other
//! request is a forward dux cannot vouch for: the address it claims is only
//! text the client may have chosen, so writing it to `config.toml` would let
//! anyone fill the blocklist with addresses of their choosing, or with the
//! owner's. Its failures are counted twice, in memory only: under the claimed
//! address, and in ONE bucket shared by all unverified traffic that came the
//! same way ([`Via`]), so a client rotating the address it claims still meets
//! the doubling wait and the global limit, while the owner signing in on this
//! machine is never held by a Funnel visitor's failures. Reaching the limit under a claimed address writes nothing; the
//! caller logs that it could not be verified and can be added by hand.
//!
//! The blocklist itself applies to EVERY address a request names (the peer,
//! each `X-Forwarded-For` entry, `X-Real-IP`, each `Forwarded: for=`): a
//! client that lies can only get itself refused, and a proxy's real client is
//! caught whichever header that proxy uses.
//!
//! This machine (a verified loopback client) is never blocked, only slowed.
//! The blocklist never matches loopback or any of this machine's own
//! addresses, and a loopback address is never blocked either, even when dux cannot vouch that
//! the request came from this machine (a forward onto loopback): blocking it
//! would lock the owner out with whoever was relayed.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use dux_core::config::{AddressBlock, ServerAuthConfig};

use super::provenance::{Classification, ClientClass, Via};

/// What failures are counted against. A verified address and a claimed one
/// are separate key spaces (decided, after review): a request can claim any
/// address it likes, so if claims shared a counter with the device really at
/// that address, an outsider could slow that device down and get it banned.
/// A verified device's wait and ban depend only on failures that came,
/// verified, from that address.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum TrackKey {
    /// An address dux verified: the only key that can lead to a ban.
    Verified(IpAddr),
    /// An address an unverified request claimed: slowed, never banned.
    Claimed(IpAddr),
    /// The one bucket every unverified request that came the same way shares.
    Unverified(Via),
}

impl TrackKey {
    /// Every key a client's failures count against: its verified address
    /// alone, or the address it claims (if any) and the shared bucket. This
    /// is the only place a key is made, so a claim can never become a
    /// verified key.
    fn of(c: &Classification) -> Vec<Self> {
        let canonical = dux_core::config_auth::canonical;
        if let Some(ip) = c.verified_ip {
            return vec![Self::Verified(canonical(ip))];
        }
        c.claimed_ip
            .map(|ip| Self::Claimed(canonical(ip)))
            .into_iter()
            .chain(std::iter::once(Self::Unverified(c.via)))
            .collect()
    }
}

/// How far dux trusts a client, which decides the global limit it shares
/// (decided, after review): unverified forwards and the internet share one,
/// verified network clients another, and verified tailnet clients and this
/// machine none, so a flood from a less trusted level never slows a more
/// trusted one.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Level {
    /// No address dux can verify, by the route it took: an unproven forward
    /// or the internet, plain loopback, or this machine's own address.
    Unverified(Via),
    /// A verified address on the network.
    Network,
    /// A verified tailnet device: per address only.
    Tailnet,
    /// This machine: per address only, and never blocked.
    ThisMachine,
}

impl Level {
    /// Whether this level shares a global per-minute limit.
    fn has_global_limit(self) -> bool {
        match self {
            Self::Unverified(_) | Self::Network => true,
            Self::Tailnet | Self::ThisMachine => false,
        }
    }

    fn of(c: &Classification) -> Self {
        match (c.class, c.verified_ip) {
            (ClientClass::ThisMachine, _) => Self::ThisMachine,
            (_, None) => Self::Unverified(c.via),
            (ClientClass::Tailnet, Some(_)) => Self::Tailnet,
            (ClientClass::Network | ClientClass::Internet, Some(_)) => Self::Network,
        }
    }
}

/// What one failure led to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strike {
    /// Nothing beyond the count and the wait.
    Counted,
    /// A verified address reached `max_failed_logins`: block it. Its count is
    /// forgotten, so a lifted block starts over.
    Block(IpAddr),
    /// An address dux could not verify reached `max_failed_logins` (this
    /// failure is the one that reached it, so it is said once per run up). It
    /// is not written anywhere; it stays slowed.
    UnverifiedLimit(IpAddr),
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
    /// The current minute of each level's global limit: when it began and
    /// how many failures it has seen.
    minutes: HashMap<Level, (Instant, u32)>,
    /// Bans that hold for this run only: their write to `config.toml` failed,
    /// or `blocked_addresses` was already at `max_blocked_addresses`.
    runtime_bans: HashSet<IpAddr>,
    /// Checks admitted and still running, per key and per level: each counts
    /// as a failure that may yet land when the next attempt's wait and limits
    /// are worked out.
    pending: HashMap<TrackKey, u32>,
    pending_minutes: HashMap<Level, u32>,
}

/// The admission state of one serve.
#[derive(Default)]
pub(crate) struct Admission {
    inner: std::sync::Arc<std::sync::Mutex<Inner>>,
}

/// An admitted check, reserved under its keys until it is settled (dropped,
/// whether it failed, succeeded or never ran). While it is held the check
/// counts as a pending failure, so a second attempt sent at the same moment
/// is the "next attempt" and waits as if this one had failed (decided, after
/// review: two guesses sent at once must not both skip the wait).
pub(crate) struct Reservation {
    inner: std::sync::Arc<std::sync::Mutex<Inner>>,
    keys: Vec<TrackKey>,
    level: Level,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for key in &self.keys {
            if let Some(n) = inner.pending.get_mut(key) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    inner.pending.remove(key);
                }
            }
        }
        if let Some(n) = inner.pending_minutes.get_mut(&self.level) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                inner.pending_minutes.remove(&self.level);
            }
        }
    }
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

    /// Whether this client is refused outright: any address it names is inside
    /// a configured `blocked_addresses` entry (`blocks`, parsed once per
    /// config) or a ban held for this run. This machine never is, and `own`
    /// (this machine's own addresses) never match.
    pub(crate) fn is_blocked(
        &self,
        blocks: &[AddressBlock],
        c: &Classification,
        own: &[IpAddr],
    ) -> bool {
        if c.verified_this_machine() {
            return false;
        }
        // Loopback never matches (decided, after review): it is this machine,
        // and `tailscale serve` relays every tailnet device from it, so an
        // entry covering it would refuse the owner and the whole tailnet. Only
        // the other addresses a request names are matched, so a serve request
        // is judged by its tailnet client's address. This machine's own
        // addresses never match either, for the same reason (decided, after
        // review): the owner's browser opening dux's LAN URL connects FROM one.
        let named = || {
            c.named
                .iter()
                .copied()
                .map(dux_core::config_auth::canonical)
                .filter(|ip| !ip.is_loopback() && !own.contains(ip))
        };
        if named().any(|ip| blocks.iter().any(|block| block.contains(ip))) {
            return true;
        }
        let inner = self.lock();
        named().any(|ip| inner.runtime_bans.contains(&ip))
    }

    /// Whether a password check may run for this client now, reserving it
    /// atomically when it may. `Err` is how long it must wait first, in whole
    /// seconds (at least one). Every check, a sign-in or a current password,
    /// is held to the same waits and limits, and a check still running counts
    /// as a failure that may yet land.
    pub(crate) fn check_attempt(
        &self,
        cfg: &ServerAuthConfig,
        c: &Classification,
        now: Instant,
    ) -> Result<Reservation, u64> {
        let window = Duration::from_secs(u64::from(cfg.failed_login_window_seconds));
        let keys = TrackKey::of(c);
        let mut inner = self.lock();
        let wait = keys
            .iter()
            .filter_map(|key| {
                let live = inner
                    .tracked
                    .get(key)
                    .filter(|failures| now.saturating_duration_since(failures.last) <= window);
                let pending = inner.pending.get(key).copied().unwrap_or(0);
                // The wait its last failure set, and the one its running
                // checks would set if they all failed now.
                let set = live
                    .filter(|failures| now < failures.next_allowed)
                    .map(|failures| failures.next_allowed - now);
                let after_pending = (pending > 0).then(|| {
                    let count = live.map_or(0, |failures| failures.count);
                    delay_after(cfg, count.saturating_add(pending))
                });
                set.into_iter()
                    .chain(after_pending)
                    .filter(|wait| !wait.is_zero())
                    .max()
            })
            .max();
        if let Some(wait) = wait {
            return Err(ceil_secs(wait));
        }
        inner.prune_minutes(now);
        let level = Level::of(c);
        if cfg.max_failed_logins_per_minute > 0 && level.has_global_limit() {
            let pending = inner.pending_minutes.get(&level).copied().unwrap_or(0);
            let (count, left) = match inner.minutes.get(&level).copied() {
                Some((start, count)) => (count, MINUTE - now.saturating_duration_since(start)),
                None => (0, MINUTE),
            };
            if count.saturating_add(pending) >= cfg.max_failed_logins_per_minute {
                return Err(ceil_secs(left));
            }
        }
        for key in &keys {
            *inner.pending.entry(*key).or_insert(0) += 1;
        }
        *inner.pending_minutes.entry(level).or_insert(0) += 1;
        Ok(Reservation {
            inner: std::sync::Arc::clone(&self.inner),
            keys,
            level,
        })
    }

    /// Count a failed check against every key the client's failures count
    /// against, and say what it led to.
    pub(crate) fn record_failure(
        &self,
        cfg: &ServerAuthConfig,
        c: &Classification,
        now: Instant,
    ) -> Strike {
        let keys = TrackKey::of(c);
        let window = Duration::from_secs(u64::from(cfg.failed_login_window_seconds));
        let mut inner = self.lock();
        inner.prune_minutes(now);
        let level = Level::of(c);
        if level.has_global_limit() {
            let minute = inner.minutes.entry(level).or_insert((now, 0));
            minute.1 = minute.1.saturating_add(1);
        }

        let mut strike = Strike::Counted;
        for key in keys {
            let count = inner.count(cfg, key, window, now);
            let (TrackKey::Verified(ip) | TrackKey::Claimed(ip)) = key else {
                continue;
            };
            if cfg.max_failed_logins == 0
                || !blockable(ip)
                || c.verified_this_machine()
                || count < cfg.max_failed_logins
            {
                continue;
            }
            match key {
                TrackKey::Verified(_) => {
                    inner.tracked.remove(&key);
                    strike = Strike::Block(ip);
                }
                TrackKey::Claimed(_) if count == cfg.max_failed_logins => {
                    strike = Strike::UnverifiedLimit(ip);
                }
                TrackKey::Claimed(_) | TrackKey::Unverified(_) => {}
            }
        }
        strike
    }

    /// Forget a client's failures after a successful check. The shared bucket
    /// is kept: one success behind an unverified forward says nothing about
    /// whoever else is guessing through it.
    pub(crate) fn record_success(&self, c: &Classification) {
        let mut inner = self.lock();
        for key in TrackKey::of(c) {
            if !matches!(key, TrackKey::Unverified(_)) {
                inner.tracked.remove(&key);
            }
        }
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
    /// Count one failure under `key` and set its next wait; answers its count
    /// in the window. Past `max_tracked_addresses`, the address whose last
    /// failure is oldest is forgotten (never the shared bucket).
    fn count(
        &mut self,
        cfg: &ServerAuthConfig,
        key: TrackKey,
        window: Duration,
        now: Instant,
    ) -> u32 {
        if !self.tracked.contains_key(&key) {
            let cap = cfg.max_tracked_addresses.max(1) as usize;
            while self.tracked.len() >= cap {
                let oldest = self
                    .tracked
                    .iter()
                    .filter(|(key, _)| !matches!(key, TrackKey::Unverified(_)))
                    .min_by_key(|(_, failures)| failures.last)
                    .map(|(key, _)| *key);
                match oldest {
                    Some(oldest) => {
                        self.tracked.remove(&oldest);
                    }
                    None => break,
                }
            }
        }
        let failures = self.tracked.entry(key).or_insert(Failures {
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
        failures.count
    }

    /// Start a new minute for each level whose current one is over.
    fn prune_minutes(&mut self, now: Instant) {
        self.minutes
            .retain(|_, (start, _)| now.saturating_duration_since(*start) < MINUTE);
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

    /// A client whose address dux verified (the direct peer).
    fn from(class: ClientClass, ip: &str) -> Classification {
        let ip: Option<IpAddr> = ip.parse().ok();
        Classification {
            class,
            named: ip.into_iter().collect(),
            verified_ip: ip,
            claimed_ip: None,
            transport_encrypted: false,
            https_serve_route: false,
            unvouched_proxy: false,
            loopback_distrusted: None,
            via: Via::Direct,
        }
    }

    impl Admission {
        /// [`Admission::check_attempt`] with the reservation given straight
        /// back, for tests about waits and limits only.
        fn check(
            &self,
            cfg: &ServerAuthConfig,
            c: &Classification,
            now: Instant,
        ) -> Result<(), u64> {
            self.check_attempt(cfg, c, now).map(drop)
        }
    }

    #[test]
    fn a_running_check_makes_the_next_attempt_wait_until_it_is_settled() {
        let a = Admission::default();
        let c = cfg();
        let t0 = Instant::now();
        let first = a
            .check_attempt(&c, &net("198.51.100.1"), t0)
            .expect("first");
        assert_eq!(
            a.check(&c, &net("198.51.100.1"), t0),
            Err(1),
            "waits as if the running one had failed"
        );
        assert_eq!(
            a.check(&c, &net("198.51.100.2"), t0),
            Ok(()),
            "only that address"
        );
        drop(first);
        assert_eq!(a.check(&c, &net("198.51.100.1"), t0), Ok(()), "settled");
        // A running check also counts toward a level's per-minute cap.
        let capped = ServerAuthConfig {
            failed_login_delay_seconds: 0,
            max_failed_logins_per_minute: 1,
            ..cfg()
        };
        let held = a
            .check_attempt(&capped, &net("198.51.100.3"), t0)
            .expect("held");
        assert!(a.check(&capped, &net("198.51.100.4"), t0).is_err());
        drop(held);
        assert_eq!(a.check(&capped, &net("198.51.100.4"), t0), Ok(()));
    }

    /// An unverified request that came by `via`.
    fn unverified(via: Via) -> Classification {
        Classification {
            via,
            ..claiming("203.0.113.1")
        }
    }

    #[test]
    fn unverified_requests_are_slowed_apart_by_the_route_they_took() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            max_failed_logins_per_minute: 1,
            ..cfg()
        };
        let t0 = Instant::now();
        let funnel = Classification {
            claimed_ip: Some("203.0.113.50".parse().unwrap()),
            ..unverified(Via::Forwarded)
        };
        a.record_failure(&c, &funnel, t0);
        assert!(a.check(&c, &unverified(Via::Forwarded), t0).is_err());
        let owner = Classification {
            claimed_ip: None,
            ..unverified(Via::PlainLoopback)
        };
        assert_eq!(a.check(&c, &owner, t0), Ok(()), "plain loopback");
        let own = Classification {
            claimed_ip: None,
            ..unverified(Via::OwnAddress)
        };
        assert_eq!(a.check(&c, &own, t0), Ok(()), "own address");
    }

    #[test]
    fn a_wrong_current_password_waits_like_a_failed_login() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            max_failed_logins: 0,
            ..cfg()
        };
        let t0 = Instant::now();
        let tailnet = from(ClientClass::Tailnet, "100.101.102.104");
        a.record_failure(&c, &tailnet, t0);
        assert!(a.check(&c, &tailnet, t0).is_err(), "slowed");
    }

    /// A forwarded request dux cannot verify, claiming `ip`.
    fn claiming(ip: &str) -> Classification {
        let ip: IpAddr = ip.parse().unwrap();
        Classification {
            class: ClientClass::Network,
            named: vec!["127.0.0.1".parse().unwrap(), ip],
            verified_ip: None,
            claimed_ip: Some(ip),
            transport_encrypted: false,
            https_serve_route: false,
            unvouched_proxy: false,
            loopback_distrusted: None,
            via: Via::Forwarded,
        }
    }

    #[test]
    fn a_claimed_address_is_slowed_and_reported_but_never_blocked() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            failed_login_delay_seconds: 0,
            max_failed_logins: 2,
            ..cfg()
        };
        let t0 = Instant::now();
        let claimed = claiming("203.0.113.9");
        assert_eq!(a.record_failure(&c, &claimed, t0), Strike::Counted);
        assert_eq!(
            a.record_failure(&c, &claimed, t0),
            Strike::UnverifiedLimit("203.0.113.9".parse().unwrap()),
            "said once, at the limit"
        );
        assert_eq!(
            a.record_failure(&c, &claimed, t0),
            Strike::Counted,
            "and not again on every failure after it"
        );
        assert!(!a.is_blocked(&[], &claimed, &[]), "nothing was banned");
    }

    #[test]
    fn a_claim_never_counts_against_the_verified_device_at_that_address() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            max_failed_logins: 2,
            max_failed_logins_per_minute: 0,
            ..cfg()
        };
        let t0 = Instant::now();
        for _ in 0..3 {
            a.record_failure(&c, &claiming("198.51.100.7"), t0);
        }
        assert_eq!(
            a.check(&c, &net("198.51.100.7"), t0),
            Ok(()),
            "the claims did not make the verified device wait"
        );
        assert_eq!(
            a.record_failure(&c, &net("198.51.100.7"), t0),
            Strike::Counted,
            "its first own failure is its first"
        );
        assert_eq!(
            a.record_failure(&c, &net("198.51.100.7"), t0 + Duration::from_secs(60)),
            Strike::Block("198.51.100.7".parse().unwrap())
        );
    }

    #[test]
    fn rotating_the_claimed_address_meets_one_shared_wait() {
        let a = Admission::default();
        let t0 = Instant::now();
        a.record_failure(&cfg(), &claiming("203.0.113.1"), t0);
        a.record_failure(&cfg(), &claiming("203.0.113.2"), t0);
        assert_eq!(
            a.check(&cfg(), &claiming("203.0.113.3"), t0),
            Err(2),
            "a fresh claim still waits out the shared, doubled wait"
        );
        assert_eq!(
            a.check(&cfg(), &net("198.51.100.1"), t0),
            Ok(()),
            "a verified client is not held by unverified traffic"
        );
    }

    #[test]
    fn the_blocklist_matches_any_address_a_request_names() {
        let a = Admission::default();
        let blocks = vec![AddressBlock::parse("203.0.113.9").unwrap()];
        assert!(a.is_blocked(&blocks, &claiming("203.0.113.9"), &[]));
        let mut leftmost = claiming("198.51.100.1");
        leftmost.named.push("203.0.113.9".parse().unwrap());
        assert!(a.is_blocked(&blocks, &leftmost, &[]));
        let own: Vec<IpAddr> = vec!["203.0.113.9".parse().unwrap()];
        assert!(
            !a.is_blocked(&blocks, &claiming("203.0.113.9"), &own),
            "this machine's own address never matches"
        );
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
        assert_eq!(a.check(&cfg(), &net("198.51.100.1"), t0), Ok(()));
        assert_eq!(
            a.record_failure(&cfg(), &net("198.51.100.1"), t0),
            Strike::Counted
        );
        assert_eq!(a.check(&cfg(), &net("198.51.100.1"), t0), Err(1));
        assert_eq!(a.check(&cfg(), &net("198.51.100.2"), t0), Ok(()));
        let later = t0 + Duration::from_millis(1001);
        assert_eq!(a.check(&cfg(), &net("198.51.100.1"), later), Ok(()));
        a.record_failure(&cfg(), &net("198.51.100.1"), later);
        assert_eq!(
            a.check(&cfg(), &net("198.51.100.1"), later),
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
        assert_eq!(a.record_failure(&c, &ip, late), Strike::Counted);
        assert_eq!(a.record_failure(&c, &ip, late), Strike::Counted);
        a.record_success(&ip);
        assert_eq!(a.record_failure(&c, &ip, late), Strike::Counted);
        assert_eq!(a.record_failure(&c, &ip, late), Strike::Counted);
        assert_eq!(
            a.record_failure(&c, &ip, late),
            Strike::Block("198.51.100.3".parse().unwrap()),
            "three in the window blocks"
        );
        assert_eq!(a.tracked(), 0, "a blocked address's count is forgotten");
    }

    /// `max_failed_logins = 0` (the setting for a proxy many people share an
    /// address behind) never blocks anyone automatically, and the slow-down
    /// still applies.
    #[test]
    fn zero_max_failed_logins_never_blocks_but_still_slows() {
        let a = Admission::default();
        let c = ServerAuthConfig {
            max_failed_logins: 0,
            ..cfg()
        };
        let ip = net("198.51.100.40");
        let mut now = Instant::now();
        for _ in 0..50 {
            assert_eq!(a.record_failure(&c, &ip, now), Strike::Counted);
            assert!(a.check(&c, &ip, now).is_err(), "slowed");
            now += Duration::from_secs(31);
        }
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
        assert_eq!(a.record_failure(&c, &machine, t0), Strike::Counted);
        // Loopback dux cannot vouch for (a forward onto it) is not blocked either.
        assert_eq!(a.record_failure(&c, &net("127.0.0.1"), t0), Strike::Counted);
        assert_eq!(a.record_failure(&c, &net("::1"), t0), Strike::Counted);
        assert_eq!(
            a.record_failure(&c, &net("::ffff:198.51.100.4"), t0),
            Strike::Block("198.51.100.4".parse().unwrap()),
            "a mapped address is blocked as the IPv4 address it carries"
        );
        let slowed = cfg();
        a.record_failure(&slowed, &machine, t0);
        assert!(
            a.check(&slowed, &machine, t0).is_err(),
            "this machine waits too"
        );
        // Nothing to block for a forwarded request with no readable address.
        let unknown = from(ClientClass::Network, "garbage");
        assert_eq!(a.record_failure(&c, &unknown, t0), Strike::Counted);
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
        assert_eq!(a.check(&c, &fresh, t0 + Duration::from_secs(10)), Err(50));
        assert_eq!(
            a.check(&c, &from(ClientClass::ThisMachine, "127.0.0.1"), t0),
            Ok(())
        );
        assert_eq!(a.check(&c, &fresh, t0 + Duration::from_secs(60)), Ok(()));
        let off = ServerAuthConfig {
            max_failed_logins_per_minute: 0,
            ..c
        };
        assert_eq!(a.check(&off, &fresh, t0), Ok(()));
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
            a.check(&c, &net("198.51.100.1"), t0 + Duration::from_millis(3)),
            Ok(()),
            "the oldest was forgotten"
        );
        assert!(a.check(&c, &net("198.51.100.3"), t0).is_err());
    }

    #[test]
    fn blocks_match_ranges_and_bans_held_for_this_run() {
        let a = Admission::default();
        let blocks = vec![AddressBlock::parse("203.0.113.0/24").unwrap()];
        assert!(a.is_blocked(&blocks, &net("203.0.113.9"), &[]));
        assert!(a.is_blocked(&blocks, &net("::ffff:203.0.113.9"), &[]));
        assert!(!a.is_blocked(&blocks, &net("198.51.100.9"), &[]));
        a.ban_for_this_run("198.51.100.9".parse().unwrap());
        assert!(a.is_blocked(&blocks, &net("198.51.100.9"), &[]));
        let everything = vec![AddressBlock::parse("0.0.0.0/0").unwrap()];
        assert!(
            !a.is_blocked(
                &everything,
                &from(ClientClass::ThisMachine, "127.0.0.1"),
                &[]
            ),
            "this machine is never refused"
        );
    }
}
