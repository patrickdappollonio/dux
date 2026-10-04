//! The web password login, server side: one optional password for one owner,
//! the same single workspace as always (no accounts, no per-user isolation).
//!
//! The pieces, each in its own module:
//!
//! - [`provenance`]: who a request is (this machine, the tailnet, the network,
//!   the internet), decided from the connection and only the headers the
//!   nearest hop can vouch for.
//! - [`admission`]: `blocked_addresses` and the failed-login counting. Applies
//!   with or without a password.
//! - [`gate`]: the bound on concurrent password checks and their queue.
//! - [`sessions`]: signed-in browsers, persisted as digests.
//! - [`cookie`]: how a session travels.
//! - [`middleware`]: the ONE layer every request passes, inside the Host guard,
//!   with the declared public routes. A no-op for authentication when no
//!   password is set (admission still applies).
//! - [`routes`]: the routes under `/api/v1/auth/`.
//! - [`socket`]: what an open WebSocket holds to be closed the moment its
//!   session ends or its client stops being allowed.
//! - [`warnings`]: what dux says about it, on every serving mode alike.
//!
//! The live `[server.auth]` section is a [`LiveAuth`], held by the engine
//! handle's live limits so every reload path (the engine actor's for
//! `dux server` and the flip, the terminal UI's for the background serve)
//! updates it in the same place, and a write dux makes itself (a password, a
//! ban, the warning's dismissal) applies to it at once.
//!
//! Proving who someone is and holding a session are separate on purpose: a
//! password is the only provider today, and another (a bearer token, an identity
//! from somewhere else) would issue sessions through [`sessions`] without the
//! middleware changing.

pub(crate) mod admission;
pub(crate) mod cookie;
pub(crate) mod gate;
pub(crate) mod middleware;
pub mod provenance;
pub(crate) mod routes;
pub(crate) mod sessions;
pub(crate) mod socket;
pub(crate) mod warnings;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use axum::http::HeaderMap;
use dux_core::auth::Password;
use dux_core::config::{AddressBlock, AuthRequire, ServerAuthConfig};
use dux_core::web_sessions::TokenDigest;

use crate::exposure::{Exposure, ExposureCell};
pub use provenance::{Arrival, ClientClass};
use provenance::{Classification, RequestFacts};
pub(crate) use socket::SocketAuth;

/// One reading of the `[server.auth]` section, with what every request needs
/// from it worked out once.
#[derive(Debug)]
pub struct AuthSnapshot {
    pub config: ServerAuthConfig,
    /// The credential generation of `config.password_hash` (empty for none).
    pub generation: String,
    /// `blocked_addresses`, parsed.
    pub blocks: Vec<AddressBlock>,
    /// Why this section cannot be used, when it cannot. A config dux loaded
    /// never has one (a bad section stops the start and is refused by a
    /// reload); this is the fail-closed answer if one ever arrives anyway.
    pub broken: Option<String>,
}

impl AuthSnapshot {
    fn of(config: &ServerAuthConfig) -> Self {
        Self {
            config: config.clone(),
            generation: dux_core::web_sessions::credential_generation(&config.password_hash),
            blocks: config.blocked(),
            broken: config.validate().err(),
        }
    }

    /// Whether a password is set.
    pub fn has_password(&self) -> bool {
        self.config.has_password()
    }
}

/// The live `[server.auth]` section. Cloning the `Arc` shares it.
#[derive(Debug)]
pub struct LiveAuth {
    current: std::sync::RwLock<Arc<AuthSnapshot>>,
    changes: tokio::sync::watch::Sender<u64>,
}

impl Default for LiveAuth {
    fn default() -> Self {
        Self::new(&ServerAuthConfig::default())
    }
}

impl LiveAuth {
    pub fn new(config: &ServerAuthConfig) -> Self {
        Self {
            current: std::sync::RwLock::new(Arc::new(AuthSnapshot::of(config))),
            changes: tokio::sync::watch::Sender::new(0),
        }
    }

    /// The section as it is now.
    pub fn snapshot(&self) -> Arc<AuthSnapshot> {
        Arc::clone(
            &self
                .current
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// Whether a password is set right now.
    pub fn has_password(&self) -> bool {
        self.snapshot().has_password()
    }

    /// Adopt a section a reload read. Answers whether anything changed.
    pub fn store(&self, config: &ServerAuthConfig) -> bool {
        {
            let mut slot = self
                .current
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.config == *config {
                return false;
            }
            *slot = Arc::new(AuthSnapshot::of(config));
        }
        self.changes.send_modify(|n| *n += 1);
        true
    }

    /// Change the section in memory after dux wrote the same change to
    /// `config.toml` itself, so it applies before the reload that follows.
    pub(crate) fn update(&self, change: impl FnOnce(&mut ServerAuthConfig)) {
        let mut config = self.snapshot().config.clone();
        change(&mut config);
        self.store(&config);
    }

    /// A receiver that wakes on every change.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }
}

/// Whether `require` asks a client of `class` for the password.
pub fn required_by(require: AuthRequire, class: ClientClass) -> bool {
    match require {
        AuthRequire::Network => matches!(class, ClientClass::Network | ClientClass::Internet),
        AuthRequire::Tailnet => !matches!(class, ClientClass::ThisMachine),
        AuthRequire::Everywhere => true,
    }
}

/// What the serve listens on, for "is dux reachable beyond this machine".
pub(crate) struct Reach {
    /// The addresses bound when the serve started.
    pub(crate) bound_ips: Vec<IpAddr>,
    /// The Tailscale leg the serve loop has bound right now, when one can come
    /// and go.
    pub(crate) tailscale_leg: Option<Arc<std::sync::Mutex<Option<SocketAddr>>>>,
}

impl Reach {
    /// The listeners beyond loopback, in words, empty when there are none.
    pub(crate) fn beyond_loopback(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .bound_ips
            .iter()
            .filter(|ip| !ip.is_loopback())
            .map(|ip| match ip {
                IpAddr::V4(v4) if v4.is_unspecified() => "every IPv4 address".to_string(),
                IpAddr::V6(v6) if v6.is_unspecified() => "every IPv6 address".to_string(),
                ip => ip.to_string(),
            })
            .collect();
        if let Some(leg) = self
            .tailscale_leg
            .as_ref()
            .and_then(|cell| *cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
            && !out.contains(&leg.ip().to_string())
        {
            out.push(leg.ip().to_string());
        }
        out
    }
}

/// What [`AuthState::start`] needs from the serve that builds it.
pub struct AuthSetup {
    pub live: Arc<LiveAuth>,
    pub exposure: Option<ExposureCell>,
    pub bound_ips: Vec<IpAddr>,
    pub tailscale_leg: Option<Arc<std::sync::Mutex<Option<SocketAddr>>>>,
    /// Where dux writes a password, a ban and the warning's dismissal. `None`
    /// for a router with no config behind it, which then writes nothing.
    pub config_path: Option<PathBuf>,
    /// The session database. `None` keeps sessions in memory only.
    pub sessions_db: Option<PathBuf>,
    pub console: crate::console::Console,
    pub engine: Option<crate::engine_actor::EngineHandle>,
    /// Asks the running dux to reload its config after dux wrote to it.
    pub reload: Arc<dyn Fn() + Send + Sync>,
}

/// The auth layer's state for one serve.
pub struct AuthState {
    live: Arc<LiveAuth>,
    exposure: Option<ExposureCell>,
    reach: Reach,
    pub(crate) sessions: sessions::Sessions,
    pub(crate) admission: admission::Admission,
    pub(crate) gate: gate::CheckGate,
    /// Bumped whenever something an open socket depends on changed: the
    /// section, a session ended, an address was blocked.
    revision: tokio::sync::watch::Sender<u64>,
    /// The generation whose password was found below the minimums at its last
    /// sign-in.
    weak: std::sync::Mutex<Option<String>>,
    proxy_warned: AtomicBool,
    config_path: Option<PathBuf>,
    reload: Arc<dyn Fn() + Send + Sync>,
    pub(crate) speaker: warnings::Speaker,
}

/// Everything the auth layer worked out about one request. Handlers read it
/// from the request's extensions.
#[derive(Debug)]
pub(crate) struct Assessment {
    pub(crate) snapshot: Arc<AuthSnapshot>,
    pub(crate) facts: RequestFacts,
    pub(crate) classification: Classification,
    /// The port in the cookie's name.
    pub(crate) cookie_port: u16,
    pub(crate) blocked: bool,
    /// Whether this request needs a session.
    pub(crate) required: bool,
    /// The valid session the request presented, if any.
    pub(crate) session: Option<TokenDigest>,
}

/// The [`Assessment`] of the request in hand, as an extension.
#[derive(Clone, Debug)]
pub(crate) struct RequestAuth(pub(crate) Arc<Assessment>);

/// What a password check came to.
#[derive(Debug)]
pub(crate) enum Verify {
    /// Right. `generation` is the one it was checked against; `weak` says it
    /// is below today's minimums.
    Right {
        generation: String,
        weak: bool,
    },
    Wrong,
    /// This failure blocked the client's address.
    Blocked,
    /// Too soon after a failure, or past the global limit; seconds to wait.
    Wait(u64),
    /// Every check slot and the queue are busy.
    Busy,
    NoPassword,
    /// The password changed while this check ran, so its answer is about a
    /// password that is no longer the one.
    Stale,
    Failed(String),
}

/// Where a blocked client is told the block lives. The full path of the file
/// is in dux's own log line, never in an answer to the client it blocks.
pub(crate) const BLOCKED_WHERE: &str =
    "blocked_addresses in the [server.auth] section of dux's config.toml";

impl AuthState {
    /// Build the state and start its background work: loading the stored
    /// sessions, writing their last use, following config changes, and the
    /// no-password warning. Needs a tokio runtime context.
    pub fn start(setup: AuthSetup) -> Arc<Self> {
        let clock = sessions::system_clock();
        Self::start_with_clock(setup, clock)
    }

    pub(crate) fn start_with_clock(setup: AuthSetup, clock: sessions::Clock) -> Arc<Self> {
        let sessions = match &setup.sessions_db {
            Some(_) => sessions::Sessions::new(clock),
            None => sessions::Sessions::in_memory(clock),
        };
        let state = Arc::new(Self {
            live: setup.live,
            exposure: setup.exposure,
            reach: Reach {
                bound_ips: setup.bound_ips,
                tailscale_leg: setup.tailscale_leg,
            },
            sessions,
            admission: admission::Admission::default(),
            gate: gate::CheckGate::default(),
            revision: tokio::sync::watch::Sender::new(0),
            weak: std::sync::Mutex::new(None),
            proxy_warned: AtomicBool::new(false),
            config_path: setup.config_path,
            reload: setup.reload,
            speaker: warnings::Speaker::new(setup.console, setup.engine),
        });
        if let Some(db) = setup.sessions_db {
            let snapshot = state.snapshot();
            let sessions = state.sessions.clone();
            tokio::spawn(async move {
                sessions
                    .load(db, snapshot.generation.clone(), idle_ms(&snapshot.config))
                    .await;
            });
        }
        tokio::spawn(maintain(Arc::clone(&state)));
        state
    }

    /// The section as it is now.
    pub fn snapshot(&self) -> Arc<AuthSnapshot> {
        self.live.snapshot()
    }

    pub(crate) fn exposure(&self) -> Exposure {
        self.exposure
            .as_ref()
            .map(ExposureCell::get)
            .unwrap_or_default()
    }

    pub(crate) fn subscribe_exposure(&self) -> Option<tokio::sync::watch::Receiver<Exposure>> {
        self.exposure.as_ref().map(ExposureCell::subscribe)
    }

    pub(crate) fn subscribe_revision(&self) -> tokio::sync::watch::Receiver<u64> {
        self.revision.subscribe()
    }

    fn bump(&self) {
        self.revision.send_modify(|n| *n += 1);
    }

    /// Work out who a request is and whether it may pass. Refreshes a valid
    /// session's last use (any request counts as activity).
    pub(crate) async fn assess(&self, facts: RequestFacts, headers: &HeaderMap) -> Assessment {
        let snapshot = self.snapshot();
        let classification = provenance::classify(&facts, &self.exposure());
        let blocked = self.admission.is_blocked(&snapshot.blocks, &classification);
        let cookie_port = facts.arrival.map_or(0, |arrival| arrival.local.port());
        let mut session = None;
        if snapshot.has_password()
            && let Some(value) = cookie::read(headers, cookie_port)
            && let Some(digest) = dux_core::web_sessions::digest_of(&value)
        {
            self.sessions.ready().await;
            if self.sessions.check(
                &digest,
                &snapshot.generation,
                idle_ms(&snapshot.config),
                true,
            ) {
                session = Some(digest);
            }
        }
        let required =
            snapshot.has_password() && required_by(snapshot.config.require, classification.class);
        Assessment {
            snapshot,
            facts,
            classification,
            cookie_port,
            blocked,
            required,
            session,
        }
    }

    /// Whether an open socket must close now, and with which code: 4403 when
    /// its client is blocked, 4401 when it needs a session it no longer has.
    pub(crate) fn socket_verdict(
        &self,
        facts: &RequestFacts,
        session: Option<&TokenDigest>,
    ) -> Option<u16> {
        let snapshot = self.snapshot();
        let classification = provenance::classify(facts, &self.exposure());
        if self.admission.is_blocked(&snapshot.blocks, &classification) {
            return Some(socket::CLOSE_BLOCKED);
        }
        if !snapshot.has_password() || !required_by(snapshot.config.require, classification.class) {
            return None;
        }
        if snapshot.broken.is_some() {
            return Some(socket::CLOSE_SIGNED_OUT);
        }
        let valid = session.is_some_and(|digest| {
            self.sessions.check(
                digest,
                &snapshot.generation,
                idle_ms(&snapshot.config),
                false,
            )
        });
        (!valid).then_some(socket::CLOSE_SIGNED_OUT)
    }

    /// Say once per run that a forwarded request arrived through a proxy dux
    /// cannot vouch for, while `require` would let such a proxy hide outsiders.
    pub(crate) fn note_proxy(&self, assessment: &Assessment) {
        if assessment.classification.unvouched_proxy
            && assessment.snapshot.config.require != AuthRequire::Everywhere
            && !self.proxy_warned.swap(true, Ordering::SeqCst)
        {
            self.speaker.proxy_warning();
        }
    }

    /// Whether dux knows it is reachable from beyond this machine: a listener
    /// beyond loopback, or a Funnel or forward it has seen.
    pub(crate) fn known_reachable(&self) -> bool {
        !self.reach.beyond_loopback().is_empty() || self.exposure().known_published()
    }

    /// The `GET /api/v1/auth/status` document for an assessed request.
    pub(crate) fn status(&self, a: &Assessment) -> routes::StatusDoc {
        let config = &a.snapshot.config;
        let password_set = a.snapshot.has_password();
        let signed_in = password_set && a.session.is_some();
        let class = a.classification.class;
        let trusted_reader = matches!(class, ClientClass::ThisMachine | ClientClass::Tailnet);
        routes::StatusDoc {
            password_set,
            required_here: a.required,
            signed_in,
            client_class: class.as_str(),
            transport_encrypted: a.classification.transport_encrypted,
            no_auth_warning: !password_set
                && !config.disable_no_auth_warning
                && (self.known_reachable() || class != ClientClass::ThisMachine),
            weak_password: password_set
                && (signed_in || !a.required)
                && self
                    .weak
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_deref()
                    == Some(a.snapshot.generation.as_str()),
            can_set_first_password: !password_set && trusted_reader,
            auth_broken: a.snapshot.broken.as_ref().map(|detail| {
                if trusted_reader {
                    detail.clone()
                } else {
                    "The [server.auth] section of dux's config is invalid.".to_string()
                }
            }),
            minimum_password_length: config.minimum_password_length,
            minimum_password_score: config.minimum_password_score,
            required_reason: a
                .classification
                .loopback_distrusted
                .filter(|_| a.required)
                .map(|cause| {
                    format!(
                        "This browser reached dux over loopback, but {cause}, so dux cannot \
                         tell it from a request relayed from elsewhere and asks for the \
                         password here too."
                    )
                }),
        }
    }

    /// Check `password` against the configured hash, through the client's
    /// slow-down, the global limit and the check gate, counting a wrong one
    /// (and blocking the address when it reaches the limit). Also measures a
    /// right one against today's minimums.
    pub(crate) async fn verify(
        &self,
        c: &Classification,
        password: Password,
        kind: admission::CheckKind,
    ) -> Verify {
        let snapshot = self.snapshot();
        let Some(hash) = snapshot.config.password_hash().map(str::to_string) else {
            return Verify::NoPassword;
        };
        if let Err(wait) = self
            .admission
            .check_attempt(&snapshot.config, c, Instant::now(), kind)
        {
            return Verify::Wait(wait);
        }
        let Ok(permit) = self
            .gate
            .enter(
                snapshot.config.max_concurrent_password_checks,
                snapshot.config.password_check_queue,
            )
            .await
        else {
            return Verify::Busy;
        };
        // A failure from this address while the check waited in the queue
        // counts before this one runs.
        if let Err(wait) = self
            .admission
            .check_attempt(&snapshot.config, c, Instant::now(), kind)
        {
            return Verify::Wait(wait);
        }
        let policy = snapshot.config.password_policy();
        let checked = tokio::task::spawn_blocking(move || {
            let outcome = dux_core::auth::verify_password(&password, &hash);
            let weak = matches!(outcome, Ok(true)) && {
                let words = dux_core::auth::guess_words();
                let words: Vec<&str> = words.iter().map(String::as_str).collect();
                !dux_core::auth::check_minimums(&password, &policy, &words).passes()
            };
            (outcome, weak)
        })
        .await;
        drop(permit);
        match checked {
            Ok((Ok(true), weak)) => {
                if self.snapshot().generation != snapshot.generation {
                    return Verify::Stale;
                }
                self.admission.record_success(c);
                Verify::Right {
                    generation: snapshot.generation.clone(),
                    weak,
                }
            }
            Ok((Ok(false), _)) => {
                if self.snapshot().generation != snapshot.generation {
                    return Verify::Stale;
                }
                match self
                    .admission
                    .record_failure(&snapshot.config, c, Instant::now())
                {
                    Some(ip) => {
                        self.block(ip).await;
                        Verify::Blocked
                    }
                    None => Verify::Wrong,
                }
            }
            Ok((Err(error), _)) => Verify::Failed(error.to_string()),
            Err(error) => Verify::Failed(format!("the password check stopped: {error}")),
        }
    }

    /// Note how the password that just signed in measures up, saying so once
    /// per password when it is below today's minimums.
    pub(crate) fn note_strength(&self, generation: &str, weak: bool) {
        let mut slot = self
            .weak
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let was = slot.as_deref() == Some(generation);
        if weak {
            *slot = Some(generation.to_string());
            if !was {
                drop(slot);
                self.speaker.weak_password();
            }
        } else if was {
            *slot = None;
        }
    }

    /// Block `ip`: append it to `blocked_addresses` through the locked config
    /// mutation, apply it at once, and say where to lift it. A write that
    /// cannot happen holds the ban in memory for this run and says that.
    pub(crate) async fn block(&self, ip: IpAddr) {
        let snapshot = self.snapshot();
        let max = snapshot.config.max_blocked_addresses;
        let failures = snapshot.config.max_failed_logins;
        // Applies before the write is even attempted: the next request from
        // this address is refused whatever the write does.
        self.admission.ban_for_this_run(ip);
        self.bump();
        let outcome = match self.config_path.clone() {
            Some(path) => tokio::task::spawn_blocking(move || {
                dux_core::config_keys::append_blocked_address(&path, ip, max)
            })
            .await
            .map_err(|e| anyhow::anyhow!("the write stopped: {e}"))
            .and_then(|written| written),
            None => Err(anyhow::anyhow!("this server has no config file")),
        };
        let path = self
            .config_path
            .as_ref()
            .map_or_else(|| "config.toml".to_string(), |p| p.display().to_string());
        match outcome {
            Ok(
                dux_core::config_keys::BanWrite::Written
                | dux_core::config_keys::BanWrite::AlreadyBlocked,
            ) => {
                // The file holds it now, so the file is where it lives: the
                // owner lifting it there (and reloading) lifts it here too.
                let entry = dux_core::config_auth::canonical(ip).to_string();
                self.live
                    .update(|config| config.blocked_addresses.push(entry));
                self.admission.lift_ban_for_this_run(ip);
                (self.reload)();
                self.speaker
                    .blocked(ip, failures, &path, warnings::BanKept::InConfig);
            }
            Ok(dux_core::config_keys::BanWrite::AtLimit) => {
                self.speaker
                    .blocked(ip, failures, &path, warnings::BanKept::ListFull { max });
            }
            Err(error) => {
                self.speaker.blocked(
                    ip,
                    failures,
                    &path,
                    warnings::BanKept::WriteFailed {
                        error: format!("{error:#}"),
                    },
                );
            }
        }
    }

    /// A session ended (sign-out): revoke it and close the sockets that held it.
    pub(crate) async fn end_session(&self, digest: TokenDigest) {
        self.sessions.revoke(digest).await;
        self.bump();
    }

    /// Where dux writes its config, when it has one.
    pub(crate) fn config_path(&self) -> Option<&PathBuf> {
        self.config_path.as_ref()
    }

    /// Adopt a change dux just wrote to `config.toml` and ask for the reload
    /// that brings the rest of the running config along.
    pub(crate) fn applied(&self, change: impl FnOnce(&mut ServerAuthConfig)) {
        self.live.update(change);
        (self.reload)();
    }
}

/// `session_idle_seconds`, in milliseconds.
pub(crate) fn idle_ms(config: &ServerAuthConfig) -> i64 {
    i64::from(config.session_idle_seconds) * 1000
}

/// How often the sessions' last use is written: a third of the idle timeout,
/// between one and fifteen seconds.
fn flush_period(config: &ServerAuthConfig) -> std::time::Duration {
    std::time::Duration::from_millis((idle_ms(config) / 3).clamp(1_000, 15_000) as u64)
}

/// How often the reach behind the no-password alarm is looked at.
const REACH_LOOK: std::time::Duration = std::time::Duration::from_secs(2);

/// The auth layer's background work, for the life of the serve's runtime.
async fn maintain(state: Arc<AuthState>) {
    let mut config = state.live.subscribe();
    let mut exposure = state.subscribe_exposure();
    let mut generation = state.snapshot().generation.clone();
    let mut warning = warnings::ExposedWarning::default();
    warning.check(&state);
    // The Tailscale leg comes and goes with no event of its own to wait on, so
    // the reach behind the no-password alarm is looked at on a short clock.
    let mut reach = tokio::time::interval(REACH_LOOK);
    reach.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Its own clock rather than a sleep made afresh each turn: the other arms
    // fire every couple of seconds, and a fresh sleep would never finish. A
    // config change that moves the period starts it again.
    let mut period = flush_period(&state.snapshot().config);
    let mut flush = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            changed = config.changed() => {
                if changed.is_err() {
                    return;
                }
                let snapshot = state.snapshot();
                if snapshot.generation != generation {
                    generation = snapshot.generation.clone();
                    state.sessions.retain_generation(&generation);
                }
                let next = flush_period(&snapshot.config);
                if next != period {
                    period = next;
                    flush = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
                    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                }
                state.bump();
                warning.check(&state);
            }
            changed = async {
                match exposure.as_mut() {
                    Some(rx) => rx.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_err() {
                    exposure = None;
                }
                warning.check(&state);
            }
            _ = flush.tick() => {
                state.sessions.flush(idle_ms(&state.snapshot().config)).await;
            }
            _ = reach.tick() => warning.check(&state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_means_exactly_what_its_comment_says() {
        use ClientClass::*;
        let table = [
            (AuthRequire::Network, [false, false, true, true]),
            (AuthRequire::Tailnet, [false, true, true, true]),
            (AuthRequire::Everywhere, [true, true, true, true]),
        ];
        for (require, wants) in table {
            for (class, want) in [ThisMachine, Tailnet, Network, Internet]
                .into_iter()
                .zip(wants)
            {
                assert_eq!(required_by(require, class), want, "{require:?} {class:?}");
            }
        }
    }

    #[test]
    fn the_live_section_reports_a_change_only_when_there_is_one() {
        let live = LiveAuth::default();
        let rx = live.subscribe();
        assert!(!live.store(&ServerAuthConfig::default()));
        assert!(!rx.has_changed().unwrap());
        live.update(|config| config.blocked_addresses.push("198.51.100.1".into()));
        assert!(rx.has_changed().unwrap());
        assert_eq!(live.snapshot().blocks.len(), 1);
    }

    /// The stored sessions' last use is written on its own clock even while
    /// the reach look ticks more often: a session kept alive by an open socket
    /// must reach the database, or a restart signs its tab out.
    #[tokio::test(start_paused = true)]
    async fn the_maintenance_loop_writes_leased_sessions_on_its_own_clock() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        let config = ServerAuthConfig {
            session_idle_seconds: 9,
            ..ServerAuthConfig::default()
        };
        let state = AuthState::start(AuthSetup {
            live: Arc::new(LiveAuth::new(&config)),
            exposure: None,
            bound_ips: Vec::new(),
            tailscale_leg: None,
            config_path: None,
            sessions_db: Some(db.clone()),
            console: crate::console::Console::noop(),
            engine: None,
            reload: Arc::new(|| {}),
        });
        state.sessions.ready().await;
        let token = state.sessions.issue("").await.unwrap();
        let _lease = state.sessions.lease(token.digest).unwrap();
        let stored = || {
            dux_core::web_sessions::WebSessionStore::open(&db)
                .unwrap()
                .load()
                .unwrap()[0]
                .last_seen_ms
        };
        let first = stored();
        // Wall time must move for the written last use to; the paused tokio
        // clock moves the loop's timers.
        std::thread::sleep(std::time::Duration::from_millis(5));
        for _ in 0..8 {
            tokio::time::advance(std::time::Duration::from_millis(500)).await;
            tokio::task::yield_now().await;
        }
        // Let the blocking write land.
        for _ in 0..50 {
            if stored() > first {
                break;
            }
            tokio::task::yield_now().await;
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            stored() > first,
            "the leased session's last use was written"
        );
    }

    #[test]
    fn the_flush_follows_the_idle_timeout_within_its_bounds() {
        let at = |seconds| {
            flush_period(&ServerAuthConfig {
                session_idle_seconds: seconds,
                ..ServerAuthConfig::default()
            })
        };
        assert_eq!(at(1).as_millis(), 1_000);
        assert_eq!(at(9).as_millis(), 3_000);
        assert_eq!(at(3600).as_millis(), 15_000);
    }
}
