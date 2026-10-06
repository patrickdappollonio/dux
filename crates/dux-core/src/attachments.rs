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
//! One thing keeps a terminal counted after its socket is gone, and one thing
//! stops counting a socket that is still there:
//!
//! - A terminal socket that was LOST (reaped, timed out, cut off by the
//!   network) while its page was being looked at (its last beat said so) keeps
//!   counting for the presence grace from that beat, whether or not the tab's
//!   events connection is still up: a phone in a pocket for minutes still
//!   protects the agent its owner is working in. Only a deliberate end
//!   releases it at once ([`Ending::Deliberate`]: a clean close, a sign-out),
//!   or the tab attaching anything again, or a change forced over it. A tab
//!   whose events connection was lost names the old one when it reconnects,
//!   and its new connection inherits what the old one still counted
//!   ([`Attachments::inherit`]).
//! - An attachment whose peer has sent nothing for [`QUIET_DEADLINE`], and
//!   whose page was not being looked at, is not counted, whatever its socket
//!   task is doing, so a socket stuck on a send to a dead peer stops blocking
//!   once the deadline passes (its socket's quiet watchdog then ends it as
//!   lost).
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

/// How a connection or an attachment ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// The client meant it: a clean close (the tab closed or went elsewhere),
    /// a sign-out, or the thing it streamed being gone. Nothing lingers.
    Deliberate,
    /// The connection was lost (reaped, timed out, cut off). A terminal its
    /// page was looking at keeps counting for the grace.
    Lost,
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
    /// The connection as it presented itself when this attached, kept here so
    /// the attachment still says who it is after its connection record goes.
    facts: Option<ConnectionFacts>,
    target: Target,
    heard: Option<Heard>,
    /// This attachment's id in the PTY ownership record, when it has one.
    pty_conn: Option<u64>,
    /// When the last beat on it said its page was being looked at; `None`
    /// when the last beat said it was not, or there was none.
    viewed_at: Option<Instant>,
}

/// A terminal whose socket was lost while its page was being looked at,
/// still counted until `until`.
struct Presence {
    connection: String,
    facts: ConnectionFacts,
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

    /// Forget a connection.
    ///
    /// Lost, only the record goes: a terminal socket linked to it ends on its
    /// own, and what the tab counted as attached to keeps counting. A
    /// deliberate end takes everything attached through it at once.
    pub fn deregister(&self, id: &str, ending: Ending) {
        self.with(|state| {
            state.connections.remove(id);
            if ending == Ending::Deliberate {
                state
                    .attachments
                    .retain(|_, attachment| attachment.connection != id);
                state.presences.retain(|presence| presence.connection != id);
            }
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
    /// holds a reservation. The token is what [`Self::detach`] takes.
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
            // The tab is attached somewhere again: whatever it was still
            // counted as attached to after losing a socket is over.
            state
                .presences
                .retain(|presence| presence.connection != connection);
            state.next += 1;
            let token = state.next;
            let facts = state
                .connections
                .get(connection)
                .map(|known| known.facts.clone());
            state.attachments.insert(
                token,
                Attachment {
                    connection: connection.to_string(),
                    facts,
                    target,
                    heard,
                    pty_conn,
                    viewed_at: None,
                },
            );
            Ok(token)
        })
    }

    /// The last beat on attachment `token` said whether its page is being
    /// looked at.
    pub fn note_viewed(&self, token: u64, viewed: bool, now: Instant) {
        self.with(|state| {
            if let Some(attachment) = state.attachments.get_mut(&token) {
                attachment.viewed_at = viewed.then_some(now);
            }
        });
    }

    /// The attachment ended. Lost while its page was being looked at, it keeps
    /// counting until `grace` after its last viewed beat; ended deliberately,
    /// nothing is left.
    pub fn detach(&self, token: u64, ending: Ending, now: Instant, grace: Duration) {
        self.with(|state| {
            let Some(attachment) = state.attachments.remove(&token) else {
                return;
            };
            if ending == Ending::Lost
                && let (Some(viewed_at), Some(facts)) = (attachment.viewed_at, attachment.facts)
                && viewed_at + grace > now
            {
                state.presences.push(Presence {
                    connection: attachment.connection,
                    facts,
                    target: attachment.target,
                    until: viewed_at + grace,
                });
            }
            state.presences.retain(|presence| presence.until > now);
        });
    }

    /// A tab's new events connection `new` takes over what its lost one `old`
    /// still counts as attached to, so the tab is not in its own way and its
    /// next attach ends it.
    ///
    /// Only from a connection that is gone: a live one keeps what it has.
    pub fn inherit(&self, old: &str, new: &str) {
        self.with(|state| {
            if state.connections.contains_key(old) {
                return;
            }
            for presence in &mut state.presences {
                if presence.connection == old {
                    presence.connection = new.to_string();
                }
            }
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
                let facts = state
                    .connections
                    .get(TERMINAL_UI_CONNECTION)
                    .map(|known| known.facts.clone());
                state.attachments.insert(
                    token,
                    Attachment {
                        connection: TERMINAL_UI_CONNECTION.to_string(),
                        facts,
                        target,
                        heard: None,
                        pty_conn,
                        viewed_at: None,
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
    /// refuse every new attachment to `scope` for `life`. A forced change also
    /// ends every lost terminal still counted in `scope`. The id is what
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
            state
                .presences
                .retain(|presence| !scope.covers(&presence.target));
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
        let exempted = |id: &str| Some(id) == exempt;
        let mut attached: Vec<(&u64, &Attachment)> = self.attachments.iter().collect();
        attached.sort_by_key(|(token, _)| **token);
        let mut blockers = Vec::new();
        for (_, attachment) in attached {
            let Some(facts) = &attachment.facts else {
                continue;
            };
            if !scope.covers(&attachment.target)
                || exempted(&attachment.connection)
                // Quiet and not being looked at: a dead peer. One that was
                // being looked at keeps counting; its socket's watchdog ends
                // it as lost, with the grace.
                || (attachment.viewed_at.is_none()
                    && attachment
                        .heard
                        .as_ref()
                        .is_some_and(|heard| heard.quiet(now)))
            {
                continue;
            }
            blockers.push(blocker(
                facts,
                &attachment.target,
                self.drives(facts, attachment),
            ));
        }
        for presence in &self.presences {
            if presence.until <= now
                || !scope.covers(&presence.target)
                || exempted(&presence.connection)
            {
                continue;
            }
            blockers.push(blocker(&presence.facts, &presence.target, false));
        }
        blockers
    }

    fn drives(&self, facts: &ConnectionFacts, attachment: &Attachment) -> bool {
        match (attachment.pty_conn, &self.owners) {
            (Some(id), Some(owners)) => owners.current_owner(&attachment.target.id).0 == Some(id),
            (Some(_), None) => false,
            // The terminal UI without a seat in an ownership record is the only
            // thing that can type into what it draws.
            (None, _) => facts.surface == Surface::TerminalUi,
        }
    }
}

fn blocker(facts: &ConnectionFacts, target: &Target, driving: bool) -> Blocker {
    Blocker {
        surface: facts.surface,
        device: facts.device.clone(),
        address: facts.address.map(|address| address.to_string()),
        verified: facts.verified,
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

        attachments.deregister("e2", Ending::Deliberate);
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

    const GRACE: Duration = Duration::from_secs(300);

    fn lose(attachments: &Attachments, token: u64, at: Instant) {
        attachments.detach(token, Ending::Lost, at, GRACE);
    }

    /// A terminal whose socket was lost (reaped, timed out, cut off) while its
    /// page was being looked at keeps counting for the grace, even once the
    /// tab's own events connection is lost too. Only a deliberate end releases
    /// it at once: a clean close, a sign-out, the tab attaching anything again
    /// (also under the id its next events connection inherits), or a change
    /// forced over it.
    #[test]
    fn a_terminal_that_was_being_looked_at_counts_for_the_grace_until_ended_deliberately() {
        type Ends = fn(&Attachments, u64, Instant);
        let cases: [(&str, Ends, bool); 8] = [
            ("its socket lost", |a, t, at| lose(a, t, at), true),
            (
                "its socket and its events connection lost",
                |a, t, at| {
                    lose(a, t, at);
                    a.deregister("phone", Ending::Lost);
                },
                true,
            ),
            (
                "its last beat not looked at",
                |a, t, at| {
                    a.note_viewed(t, false, at);
                    lose(a, t, at);
                },
                false,
            ),
            (
                "its socket closed cleanly",
                |a, t, at| {
                    a.detach(t, Ending::Deliberate, at, GRACE);
                },
                false,
            ),
            (
                "signed out after its socket was lost",
                |a, t, at| {
                    lose(a, t, at);
                    a.deregister("phone", Ending::Deliberate);
                },
                false,
            ),
            (
                "the tab attached elsewhere",
                |a, t, at| {
                    lose(a, t, at);
                    a.attach("phone", tab("s9-slot", "s9"), None, None).unwrap();
                },
                false,
            ),
            (
                "the tab attached elsewhere from its next events connection",
                |a, t, at| {
                    lose(a, t, at);
                    a.deregister("phone", Ending::Lost);
                    a.register("phone-again", browser("100.64.0.9", "Safari"), None);
                    a.inherit("phone", "phone-again");
                    a.attach("phone-again", tab("s9-slot", "s9"), None, None)
                        .unwrap();
                },
                false,
            ),
            (
                "a change forced over it",
                |a, t, at| {
                    lose(a, t, at);
                    let forced = Policy {
                        requester: None,
                        force: true,
                    };
                    let id = a
                        .reserve(agent_scope("s1"), &forced, Life::Released, at)
                        .unwrap();
                    a.release(id);
                },
                false,
            ),
        ];
        let beat = Instant::now();
        for (case, end, blocks) in cases {
            let attachments = Attachments::default();
            attachments.register(
                "phone",
                browser("100.64.0.9", "Safari"),
                Some(Heard::at(beat)),
            );
            let token = attachments
                .attach("phone", tab("s1-slot", "s1"), None, None)
                .unwrap();
            attachments.note_viewed(token, true, beat);
            end(&attachments, token, beat + Duration::from_secs(10));
            let blockers =
                attachments.blockers(&agent_scope("s1"), None, beat + Duration::from_secs(60));
            assert_eq!(!blockers.is_empty(), blocks, "{case}: {blockers:?}");
        }
    }

    /// The grace runs from the socket's last viewed beat, not from when the
    /// socket was lost, and a tab's next events connection that inherits it is
    /// not in its own way.
    #[test]
    fn the_grace_runs_from_the_last_viewed_beat() {
        let attachments = Attachments::default();
        let beat = Instant::now();
        attachments.register("phone", browser("100.64.0.9", "Safari"), None);
        let token = attachments
            .attach("phone", tab("s1-slot", "s1"), None, None)
            .unwrap();
        attachments.note_viewed(token, true, beat);
        lose(&attachments, token, beat + Duration::from_secs(100));
        let at = |offset: u64| beat + Duration::from_secs(offset);
        assert_eq!(
            attachments
                .blockers(&agent_scope("s1"), None, at(299))
                .len(),
            1
        );
        assert!(
            attachments
                .blockers(&agent_scope("s1"), None, at(301))
                .is_empty()
        );

        attachments.deregister("phone", Ending::Lost);
        attachments.inherit("phone", "phone-again");
        assert!(
            attachments
                .blockers(&agent_scope("s1"), Some("phone-again"), at(60))
                .is_empty()
        );
    }

    /// An attachment whose peer went quiet stops blocking at the deadline on
    /// its own, whatever its socket is stuck doing.
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
