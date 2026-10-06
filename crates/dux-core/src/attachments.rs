//! Who is attached to what: the one registry every destructive change asks
//! before it cuts somebody's terminal off.
//!
//! A **connection** is one client: a browser tab (its events socket, or a
//! terminal socket that could not be linked to one), or the terminal UI. An
//! **attachment** is that connection streaming one agent tab or terminal. A
//! browser tab's terminal sockets name the tab's events connection when they
//! open, so a tab and every terminal it shows are one connection here.
//!
//! A change that would end terminals (deleting or stopping an agent, a forced
//! restart, closing a tab, closing a terminal, removing a project) asks
//! [`Attachments::reserve`] in one locked step: it either answers who is in
//! the way, or installs a **reservation** that refuses every new attachment to
//! what it covers until the change has finished. An attach and a reservation
//! take the same lock, so of an attach and a reservation racing, exactly one
//! wins.
//!
//! Only the asking connection's own attachments are exempt: the terminal UI
//! deleting what it shows, or the browser tab that sent the request. Driving
//! or watching is read from the PTY ownership record, never from a client.
//!
//! Two things keep a connection counted that is not streaming right now, and
//! one thing stops counting one that is:
//!
//! - A browser tab whose terminal socket closed while its page was being
//!   looked at (the last beat on it said so) still counts as attached to that
//!   terminal for the presence grace, as long as its events connection is
//!   live: a phone whose screen went off for a minute still protects the agent
//!   its owner is working in.
//! - A connection whose peer has sent nothing for [`QUIET_DEADLINE`] is not
//!   counted, whatever its socket task is doing, so a socket stuck on a send
//!   to a dead peer stops blocking anything once the deadline passes.
//!
//! In memory only: a restart loses every connection, so it loses every
//! attachment too, and a stored one would block deletes after a crash.

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::operations::RecordWatch;
use crate::pty_owners::PtySizeOwners;

/// How long a peer may send nothing at all, not even the pong every browser
/// answers a ping with, before its connection stops counting: two missed
/// 30-second pings and some slack. The web server closes such a socket at the
/// same deadline.
pub const QUIET_DEADLINE: Duration = Duration::from_secs(75);

/// The terminal UI's connection id. One per process: this process is one
/// device.
pub const TERMINAL_UI_CONNECTION: &str = "terminal-ui";

/// Which kind of client a connection is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Browser,
    TerminalUi,
}

/// What an attachment streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// An agent's tab (its first tab included).
    Tab,
    /// A terminal: an agent's, a project's or a standalone one.
    Terminal,
}

/// One agent tab or terminal, with the agent it belongs to when it belongs to
/// one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Target {
    pub kind: TargetKind,
    /// The PTY's id: the tab id or the terminal id.
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// Who a connection is, as it presented itself when it connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionFacts {
    pub surface: Surface,
    /// The raw `User-Agent` a browser sent (length-bounded by the web layer),
    /// or the terminal UI's fixed label.
    pub device: Option<String>,
    /// The client's address: the verified one when the auth layer could
    /// verify it, otherwise the peer's.
    pub address: Option<IpAddr>,
    pub verified: bool,
    /// Whether this is a browser tab's events connection, the only kind a
    /// terminal socket may link itself to.
    pub events: bool,
}

/// When a peer was last heard from. Shared between the socket that hears it
/// and this registry, which reads it rather than waiting for the socket's own
/// loop to notice the silence.
#[derive(Clone, Debug)]
pub struct Heard(Arc<Mutex<Instant>>);

impl Heard {
    pub fn now() -> Self {
        Self::at(Instant::now())
    }

    pub fn at(at: Instant) -> Self {
        Self(Arc::new(Mutex::new(at)))
    }

    /// The peer just sent something.
    pub fn touch(&self) {
        *self.lock() = Instant::now();
    }

    /// How long the peer has been silent at `now`.
    pub fn quiet_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(*self.lock())
    }

    fn quiet(&self, now: Instant) -> bool {
        self.quiet_for(now) > QUIET_DEADLINE
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Instant> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What a destructive change would cut off: these PTYs, and anything attached
/// to these agents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub ptys: BTreeSet<String>,
    pub agents: BTreeSet<String>,
}

impl Scope {
    fn covers(&self, target: &Target) -> bool {
        self.ptys.contains(&target.id)
            || target
                .agent
                .as_ref()
                .is_some_and(|agent| self.agents.contains(agent))
    }
}

/// Who is asking for a destructive change, and whether they said to go ahead
/// over everybody attached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// The asking connection, whose own attachments never block it. `None`
    /// for a client with no connection here (the command line).
    pub requester: Option<String>,
    /// Skip the refusal. The reservation is still installed.
    pub force: bool,
}

/// How long a reservation lasts.
#[derive(Clone, Debug)]
pub enum Life {
    /// Until this operation record finishes.
    Record(RecordWatch),
    /// Until [`Attachments::release`].
    Released,
}

/// One connection in the way of a change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub surface: Surface,
    pub device: Option<String>,
    pub address: Option<String>,
    pub verified: bool,
    pub driving: bool,
    pub target: Target,
}

/// A new attachment refused because a change is ending what it would attach
/// to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reserved;

struct Connection {
    facts: ConnectionFacts,
    heard: Option<Heard>,
}

struct Attachment {
    connection: String,
    target: Target,
    heard: Option<Heard>,
    /// This attachment's id in the PTY ownership record, when it has one.
    pty_conn: Option<u64>,
    /// Whether the last beat on it said its page was being looked at.
    viewed: bool,
}

/// An attachment that ended while it was being looked at, still counted until
/// `until` while its connection is live.
struct Presence {
    connection: String,
    target: Target,
    until: Instant,
}

struct Reservation {
    scope: Scope,
    life: Life,
}

impl Reservation {
    fn live(&self) -> bool {
        match &self.life {
            Life::Record(watch) => watch.is_open(),
            Life::Released => true,
        }
    }
}

#[derive(Default)]
struct State {
    connections: HashMap<String, Connection>,
    attachments: HashMap<u64, Attachment>,
    presences: Vec<Presence>,
    reservations: HashMap<u64, Reservation>,
    next: u64,
    owners: Option<Arc<PtySizeOwners>>,
}

/// The registry. Cheaply cloned; every clone is the same registry.
#[derive(Clone, Default)]
pub struct Attachments(Arc<Mutex<State>>);

impl std::fmt::Debug for Attachments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Attachments").finish()
    }
}

impl Attachments {
    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// The PTY ownership record driving is read from. Set by each serve, whose
    /// record it is.
    pub fn use_owners(&self, owners: Arc<PtySizeOwners>) {
        self.with(|state| state.owners = Some(owners));
    }

    /// Record a connection. `heard` is `None` for one that never goes quiet
    /// (the terminal UI).
    pub fn register(&self, id: &str, facts: ConnectionFacts, heard: Option<Heard>) {
        self.with(|state| {
            state
                .connections
                .insert(id.to_string(), Connection { facts, heard });
        });
    }

    /// Forget a connection with everything attached through it.
    pub fn deregister(&self, id: &str) {
        self.with(|state| {
            state.connections.remove(id);
            state
                .attachments
                .retain(|_, attachment| attachment.connection != id);
            state.presences.retain(|presence| presence.connection != id);
        });
    }

    /// Whether a terminal socket from `address` may count as part of the
    /// events connection `id`: one that is live, is an events connection, and
    /// came from the same address.
    pub fn linkable(&self, id: &str, address: Option<IpAddr>, now: Instant) -> bool {
        self.with(|state| {
            state.connections.get(id).is_some_and(|connection| {
                connection.facts.events
                    && connection.facts.address == address
                    && connection_live(connection, now)
            })
        })
    }

    /// Attach `connection` to `target`, refused while a change that covers it
    /// holds a reservation. The token is what [`Self::detach`] and
    /// [`Self::revoke`] take.
    pub fn attach(
        &self,
        connection: &str,
        target: Target,
        heard: Option<Heard>,
        pty_conn: Option<u64>,
    ) -> Result<u64, Reserved> {
        self.with(|state| {
            state.prune_reservations();
            if state.reserved(&target) {
                return Err(Reserved);
            }
            state.next += 1;
            let token = state.next;
            state.attachments.insert(
                token,
                Attachment {
                    connection: connection.to_string(),
                    target,
                    heard,
                    pty_conn,
                    viewed: false,
                },
            );
            Ok(token)
        })
    }

    /// The last beat on attachment `token` said whether its page is being
    /// looked at.
    pub fn note_viewed(&self, token: u64, viewed: bool) {
        self.with(|state| {
            if let Some(attachment) = state.attachments.get_mut(&token) {
                attachment.viewed = viewed;
            }
        });
    }

    /// The attachment ended. When its page was being looked at, it keeps
    /// counting for `grace` while its connection is live.
    pub fn detach(&self, token: u64, now: Instant, grace: Duration) {
        self.with(|state| {
            let Some(attachment) = state.attachments.remove(&token) else {
                return;
            };
            if attachment.viewed && !grace.is_zero() {
                state.presences.push(Presence {
                    connection: attachment.connection,
                    target: attachment.target,
                    until: now + grace,
                });
            }
            state.presences.retain(|presence| presence.until > now);
        });
    }

    /// The attachment's peer went quiet: forget it at once, with no grace.
    pub fn revoke(&self, token: u64) {
        self.with(|state| {
            state.attachments.remove(&token);
        });
    }

    /// Replace everything the terminal UI is attached to with `targets`,
    /// skipping any a change is ending. `pty_conn` is the terminal UI's id in
    /// the PTY ownership record while it has one.
    pub fn set_terminal_ui(&self, targets: Vec<Target>, pty_conn: Option<u64>) {
        self.with(|state| {
            state
                .attachments
                .retain(|_, attachment| attachment.connection != TERMINAL_UI_CONNECTION);
            state.prune_reservations();
            for target in targets {
                if state.reserved(&target) {
                    continue;
                }
                state.next += 1;
                let token = state.next;
                state.attachments.insert(
                    token,
                    Attachment {
                        connection: TERMINAL_UI_CONNECTION.to_string(),
                        target,
                        heard: None,
                        pty_conn,
                        viewed: false,
                    },
                );
            }
        });
    }

    /// Who would be cut off by a change to `scope`, leaving out `exempt`.
    pub fn blockers(&self, scope: &Scope, exempt: Option<&str>, now: Instant) -> Vec<Blocker> {
        self.with(|state| state.blockers(scope, exempt, now))
    }

    /// In one step: refuse a change to `scope` with the connections in its
    /// way, unless there are none or the policy forces it, and otherwise
    /// refuse every new attachment to `scope` for `life`. The id is what
    /// [`Self::release`] takes.
    pub fn reserve(
        &self,
        scope: Scope,
        policy: &Policy,
        life: Life,
        now: Instant,
    ) -> Result<u64, Vec<Blocker>> {
        self.with(|state| {
            let blockers = state.blockers(&scope, policy.requester.as_deref(), now);
            if !blockers.is_empty() && !policy.force {
                return Err(blockers);
            }
            state.prune_reservations();
            state.next += 1;
            let id = state.next;
            state.reservations.insert(id, Reservation { scope, life });
            Ok(id)
        })
    }

    /// End a reservation.
    pub fn release(&self, id: u64) {
        self.with(|state| {
            state.reservations.remove(&id);
        });
    }
}

fn connection_live(connection: &Connection, now: Instant) -> bool {
    connection
        .heard
        .as_ref()
        .is_none_or(|heard| !heard.quiet(now))
}

impl State {
    fn prune_reservations(&mut self) {
        self.reservations
            .retain(|_, reservation| reservation.live());
    }

    fn reserved(&self, target: &Target) -> bool {
        self.reservations
            .values()
            .any(|reservation| reservation.scope.covers(target))
    }

    fn blockers(&self, scope: &Scope, exempt: Option<&str>, now: Instant) -> Vec<Blocker> {
        let counted = |id: &str| -> Option<&Connection> {
            if Some(id) == exempt {
                return None;
            }
            self.connections
                .get(id)
                .filter(|connection| connection_live(connection, now))
        };
        let mut attached: Vec<(&u64, &Attachment)> = self.attachments.iter().collect();
        attached.sort_by_key(|(token, _)| **token);
        let mut blockers = Vec::new();
        for (_, attachment) in attached {
            if !scope.covers(&attachment.target)
                || attachment
                    .heard
                    .as_ref()
                    .is_some_and(|heard| heard.quiet(now))
            {
                continue;
            }
            let Some(connection) = counted(&attachment.connection) else {
                continue;
            };
            blockers.push(blocker(
                connection,
                &attachment.target,
                self.drives(connection, attachment),
            ));
        }
        for presence in &self.presences {
            if presence.until <= now || !scope.covers(&presence.target) {
                continue;
            }
            if let Some(connection) = counted(&presence.connection) {
                blockers.push(blocker(connection, &presence.target, false));
            }
        }
        blockers
    }

    fn drives(&self, connection: &Connection, attachment: &Attachment) -> bool {
        match (attachment.pty_conn, &self.owners) {
            (Some(id), Some(owners)) => owners.current_owner(&attachment.target.id).0 == Some(id),
            (Some(_), None) => false,
            // The terminal UI without a seat in an ownership record is the only
            // thing that can type into what it draws.
            (None, _) => connection.facts.surface == Surface::TerminalUi,
        }
    }
}

fn blocker(connection: &Connection, target: &Target, driving: bool) -> Blocker {
    Blocker {
        surface: connection.facts.surface,
        device: connection.facts.device.clone(),
        address: connection.facts.address.map(|address| address.to_string()),
        verified: connection.facts.verified,
        driving,
        target: target.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::{OperationKind, OperationPolicy, Operations};

    fn browser(address: &str, device: &str) -> ConnectionFacts {
        ConnectionFacts {
            surface: Surface::Browser,
            device: Some(device.to_string()),
            address: Some(address.parse().unwrap()),
            verified: false,
            events: true,
        }
    }

    fn tab(id: &str, agent: &str) -> Target {
        Target {
            kind: TargetKind::Tab,
            id: id.to_string(),
            agent: Some(agent.to_string()),
        }
    }

    fn agent_scope(agent: &str) -> Scope {
        Scope {
            ptys: BTreeSet::new(),
            agents: [agent.to_string()].into(),
        }
    }

    fn cli() -> Policy {
        Policy::default()
    }

    /// A change to an agent a browser streams is refused with that browser,
    /// as it presented itself; forced, it goes ahead and reserves the agent,
    /// so the browser cannot attach again while the change runs.
    #[test]
    fn a_change_is_refused_with_who_is_attached_and_a_forced_one_reserves() {
        let attachments = Attachments::default();
        let now = Instant::now();
        attachments.register(
            "e1",
            browser("192.168.1.5", "Firefox"),
            Some(Heard::at(now)),
        );
        attachments
            .attach("e1", tab("s1-slot", "s1"), Some(Heard::at(now)), Some(7))
            .unwrap();

        let refused = attachments
            .reserve(agent_scope("s1"), &cli(), Life::Released, now)
            .unwrap_err();
        assert_eq!(
            refused,
            vec![Blocker {
                surface: Surface::Browser,
                device: Some("Firefox".to_string()),
                address: Some("192.168.1.5".to_string()),
                verified: false,
                driving: false,
                target: tab("s1-slot", "s1"),
            }]
        );
        assert!(
            attachments
                .reserve(agent_scope("s2"), &cli(), Life::Released, now)
                .is_ok(),
            "another agent has nobody attached"
        );

        let forced = Policy {
            requester: None,
            force: true,
        };
        let id = attachments
            .reserve(agent_scope("s1"), &forced, Life::Released, now)
            .expect("forced");
        assert_eq!(
            attachments.attach("e2", tab("s1-tab2", "s1"), None, None),
            Err(Reserved)
        );
        attachments.release(id);
        assert!(
            attachments
                .attach("e2", tab("s1-tab2", "s1"), None, None)
                .is_ok()
        );
    }

    /// Driving is whatever the ownership record says about this attachment's
    /// own id there.
    #[test]
    fn a_blocker_drives_when_the_ownership_record_names_it() {
        let attachments = Attachments::default();
        let owners = Arc::new(PtySizeOwners::default());
        attachments.use_owners(Arc::clone(&owners));
        let now = Instant::now();
        attachments.register("e1", browser("10.0.0.2", "Chrome"), None);
        attachments
            .attach("e1", tab("s1-slot", "s1"), None, Some(41))
            .unwrap();
        owners.claim("s1-slot", 41);

        let blockers = attachments.blockers(&agent_scope("s1"), None, now);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].driving);
    }

    /// Only the asking connection is exempt: a second browser tab on the same
    /// machine, with the same address and the same `User-Agent`, still blocks.
    #[test]
    fn only_the_requesting_connection_is_exempt() {
        let attachments = Attachments::default();
        let now = Instant::now();
        for id in ["e1", "e2"] {
            attachments.register(id, browser("127.0.0.1", "Safari"), None);
            attachments
                .attach(id, tab("s1-slot", "s1"), None, None)
                .unwrap();
        }
        let own = Policy {
            requester: Some("e1".to_string()),
            force: false,
        };
        let refused = attachments
            .reserve(agent_scope("s1"), &own, Life::Released, now)
            .unwrap_err();
        assert_eq!(refused.len(), 1, "{refused:?}");

        attachments.deregister("e2");
        assert!(
            attachments
                .reserve(agent_scope("s1"), &own, Life::Released, now)
                .is_ok()
        );
    }

    /// A terminal socket may link itself only to a live events connection
    /// from its own address.
    #[test]
    fn a_terminal_socket_links_only_to_a_live_events_connection_from_its_own_address() {
        let attachments = Attachments::default();
        let now = Instant::now();
        attachments.register("e1", browser("10.0.0.2", "Chrome"), Some(Heard::at(now)));
        let mut pty_only = browser("10.0.0.2", "Chrome");
        pty_only.events = false;
        attachments.register("p1", pty_only, None);

        assert!(attachments.linkable("e1", Some("10.0.0.2".parse().unwrap()), now));
        assert!(!attachments.linkable("e1", Some("10.0.0.3".parse().unwrap()), now));
        assert!(!attachments.linkable("p1", Some("10.0.0.2".parse().unwrap()), now));
        assert!(!attachments.linkable("nobody", Some("10.0.0.2".parse().unwrap()), now));
        assert!(
            !attachments.linkable(
                "e1",
                Some("10.0.0.2".parse().unwrap()),
                now + QUIET_DEADLINE + Duration::from_secs(1)
            ),
            "a quiet events connection is no longer live"
        );
    }

    /// A terminal socket that closed while its page was looked at keeps its
    /// tab counted for the grace, while the tab's events connection is live;
    /// one that closed unlooked-at does not linger at all.
    #[test]
    fn a_closed_socket_that_was_being_looked_at_counts_for_the_grace() {
        let attachments = Attachments::default();
        let now = Instant::now();
        let grace = Duration::from_secs(300);
        attachments.register(
            "phone",
            browser("100.64.0.9", "Safari"),
            Some(Heard::at(now)),
        );
        let viewed = attachments
            .attach("phone", tab("s1-slot", "s1"), None, None)
            .unwrap();
        attachments.note_viewed(viewed, true);
        attachments.detach(viewed, now, grace);
        let unviewed = attachments
            .attach("phone", tab("s2-slot", "s2"), None, None)
            .unwrap();
        attachments.detach(unviewed, now, grace);

        assert_eq!(
            attachments
                .blockers(&agent_scope("s1"), None, now + Duration::from_secs(60))
                .len(),
            1,
            "inside the grace"
        );
        assert!(
            attachments
                .blockers(&agent_scope("s2"), None, now + Duration::from_secs(60))
                .is_empty()
        );
        assert!(
            attachments
                .blockers(
                    &agent_scope("s1"),
                    None,
                    now + grace + Duration::from_secs(1)
                )
                .is_empty(),
            "past the grace"
        );
        attachments.deregister("phone");
        assert!(
            attachments
                .blockers(&agent_scope("s1"), None, now + Duration::from_secs(60))
                .is_empty(),
            "the events connection is gone"
        );
    }

    /// An attachment whose peer went quiet stops blocking at the deadline on
    /// its own, whatever its socket is stuck doing; one revoked is gone and
    /// leaves no grace behind.
    #[test]
    fn a_quiet_attachment_stops_blocking_at_the_deadline() {
        let attachments = Attachments::default();
        let now = Instant::now();
        attachments.register("e1", browser("10.0.0.2", "Chrome"), None);
        let heard = Heard::at(now);
        attachments
            .attach("e1", tab("s1-slot", "s1"), Some(heard), None)
            .unwrap();
        assert_eq!(
            attachments
                .blockers(&agent_scope("s1"), None, now + Duration::from_secs(74))
                .len(),
            1
        );
        assert!(
            attachments
                .blockers(&agent_scope("s1"), None, now + Duration::from_secs(76))
                .is_empty()
        );

        let token = attachments
            .attach("e1", tab("s2-slot", "s2"), None, None)
            .unwrap();
        attachments.note_viewed(token, true);
        attachments.revoke(token);
        attachments.detach(token, now, Duration::from_secs(300));
        assert!(
            attachments
                .blockers(&agent_scope("s2"), None, now)
                .is_empty()
        );
    }

    /// Of an attach and a reservation racing for the same agent, exactly one
    /// wins: either the attach lands and the change is refused with it, or
    /// the change reserves and the attach is refused.
    #[test]
    fn of_an_attach_and_a_reserve_racing_exactly_one_wins() {
        for _ in 0..200 {
            let attachments = Attachments::default();
            attachments.register("e1", browser("10.0.0.2", "Chrome"), None);
            let gate = Arc::new(std::sync::Barrier::new(2));
            let attach = {
                let attachments = attachments.clone();
                let gate = Arc::clone(&gate);
                std::thread::spawn(move || {
                    gate.wait();
                    attachments.attach("e1", tab("s1-slot", "s1"), None, None)
                })
            };
            let reserve = {
                let attachments = attachments.clone();
                let gate = Arc::clone(&gate);
                std::thread::spawn(move || {
                    gate.wait();
                    attachments.reserve(agent_scope("s1"), &cli(), Life::Released, Instant::now())
                })
            };
            let attached = attach.join().unwrap().is_ok();
            let reserved = reserve.join().unwrap().is_ok();
            assert!(
                attached != reserved,
                "attached {attached}, reserved {reserved}"
            );
        }
    }

    /// A reservation tied to an operation record lasts exactly as long as the
    /// record is open, even when the record moves to another id.
    #[test]
    fn a_reservation_tied_to_a_record_ends_when_the_record_finishes() {
        let operations = Operations::default();
        let now = Instant::now();
        operations.open(
            "op-1",
            OperationKind::AgentDelete,
            OperationPolicy::from_server(&crate::config::ServerConfig::default()),
            now,
        );
        let watch = operations.watch("op-1").expect("open record");
        operations.rekey("op-1", "op-2");
        let attachments = Attachments::default();
        attachments
            .reserve(agent_scope("s1"), &cli(), Life::Record(watch), now)
            .unwrap();
        assert_eq!(
            attachments.attach("e1", tab("s1-slot", "s1"), None, None),
            Err(Reserved)
        );
        operations.finish(
            "op-2",
            crate::statusline::StatusTone::Info,
            "done",
            None,
            now,
        );
        assert!(
            attachments
                .attach("e1", tab("s1-slot", "s1"), None, None)
                .is_ok()
        );
    }

    /// The terminal UI's attachments are exactly the panes it last drew, and
    /// with no ownership seat it drives them.
    #[test]
    fn the_terminal_ui_is_attached_to_exactly_what_it_last_drew() {
        let attachments = Attachments::default();
        let now = Instant::now();
        attachments.register(
            TERMINAL_UI_CONNECTION,
            ConnectionFacts {
                surface: Surface::TerminalUi,
                device: Some("the dux TUI".to_string()),
                address: None,
                verified: true,
                events: false,
            },
            None,
        );
        attachments.set_terminal_ui(vec![tab("s1-slot", "s1")], None);
        let blockers = attachments.blockers(&agent_scope("s1"), None, now);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].driving);
        assert_eq!(blockers[0].surface, Surface::TerminalUi);

        attachments.set_terminal_ui(vec![tab("s2-slot", "s2")], None);
        assert!(
            attachments
                .blockers(&agent_scope("s1"), None, now)
                .is_empty()
        );
        assert!(
            attachments
                .blockers(&agent_scope("s2"), Some(TERMINAL_UI_CONNECTION), now)
                .is_empty(),
            "the terminal UI never blocks itself"
        );
    }
}
