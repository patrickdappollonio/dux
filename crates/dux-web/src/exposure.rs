//! What dux knows about how far beyond its own listeners it is published: a
//! Tailscale Funnel, a raw TCP forward onto its port, and the `tailscale serve`
//! routes that end at it.
//!
//! dux used to refuse every request while it saw (or could not rule out) a
//! Funnel to its port, because it had no login. With the optional password,
//! that knowledge feeds request classification instead
//! (`crate::auth::provenance`):
//!
//! - While anything might hand dux a bare stream from loopback that nobody on
//!   this machine sent (a Funnel to dux, a raw TCP forward to dux, or a state in
//!   which nobody knows yet), a loopback request with no trustworthy origin is
//!   classed as the NETWORK, so a configured password applies to it.
//! - A forwarded loopback request counts as the tailnet only through a
//!   confirmed, non-Funnel `tailscale serve` route to dux whose name and port
//!   its `Host` matches.
//! - With no password, dux serves whatever it sees and says loudly what that
//!   exposes (`crate::auth::warnings`): a risky configuration is the owner's
//!   call.
//!
//! The serve loop is the only writer of an [`ExposureCell`]; the Host guard,
//! the auth layer and every open socket read it. It is a watch channel so a
//! socket that classified its client when it opened is told the moment the
//! answer may have changed.

use std::sync::Arc;

use dux_core::tailscale::{ServeRoute, TailscaleIdentity};

/// What dux knows about a Tailscale Funnel to its port.
///
/// Every state but [`FunnelState::Open`] is one in which a request from
/// loopback may have come from the public internet, so such a request is not
/// taken for this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunnelState {
    /// No Funnel forwards to dux, as far as the last look could tell, or dux
    /// is not consulting Tailscale at all (`tailscale = "no"`).
    Open,
    /// The serve has started and its first look at Tailscale has not answered
    /// yet. Unknown is not clear.
    Checking,
    /// The Tailscale CLI is there but failed or did not answer, so nobody knows
    /// whether a Funnel publishes dux.
    Unconfirmed,
    /// Tailscale is evidently on this machine but its CLI is not where dux
    /// looks, so nobody knows whether a Funnel publishes dux.
    CliNotFound,
    /// A Funnel forwards to dux's port.
    Funnel,
    /// The node is down, but its saved configuration Funnels dux's port, which
    /// comes back the moment it is brought up.
    FunnelSaved,
}

/// What one successful look at Tailscale says about the ways in, kept beside
/// the [`FunnelState`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentityFacts {
    /// A Funnel is on for anything on this machine, whatever it forwards to.
    pub funnel_any: bool,
    /// A raw TCP forward (Funnel or not) names dux's port.
    pub forward_to_dux: bool,
    /// The `tailscale serve` web routes that end at dux.
    pub routes: Vec<ServeRoute>,
}

impl IdentityFacts {
    /// The facts a look's identity carries.
    pub fn of(identity: &TailscaleIdentity) -> Self {
        Self {
            funnel_any: identity.funnel,
            forward_to_dux: identity.forward_to_dux,
            routes: identity.serve.clone(),
        }
    }
}

/// Everything the serve knows about its exposure right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exposure {
    pub funnel: FunnelState,
    /// The last successful look's facts, or `None` when no look is current (none
    /// has landed, the last one failed, or dux does not consult Tailscale).
    pub identity: Option<IdentityFacts>,
}

impl Default for Exposure {
    /// A serve that does not consult Tailscale, or no serve at all (a test
    /// router): nothing known that publishes dux.
    fn default() -> Self {
        Self {
            funnel: FunnelState::Open,
            identity: None,
        }
    }
}

impl Exposure {
    /// Whether a loopback request with no trustworthy origin may have come from
    /// somewhere other than this machine: any Funnel state but open, or a raw
    /// TCP forward onto dux's port.
    pub fn loopback_possibly_public(&self) -> bool {
        self.funnel != FunnelState::Open
            || self
                .identity
                .as_ref()
                .is_some_and(|facts| facts.forward_to_dux)
    }

    /// Whether dux KNOWS it is published beyond this machine and its own
    /// listeners: a Funnel to it (live or saved) or a raw forward onto its port.
    /// Unknown states are not counted here; they are what classification fails
    /// closed on, not what a warning claims as a fact.
    pub fn known_published(&self) -> bool {
        matches!(self.funnel, FunnelState::Funnel | FunnelState::FunnelSaved)
            || self
                .identity
                .as_ref()
                .is_some_and(|facts| facts.forward_to_dux)
    }

    /// Whether a Funnel is on for anything on this machine.
    pub fn funnel_any(&self) -> bool {
        self.identity.as_ref().is_some_and(|facts| facts.funnel_any)
    }

    /// The confirmed, non-Funnel `tailscale serve` route to dux that a request
    /// with this `Host` header came through, if any: its name, and its port
    /// (the scheme's default when the header names none), must both match.
    pub fn confirmed_route(&self, host_header: &str) -> Option<&ServeRoute> {
        let facts = self.identity.as_ref()?;
        let (host, port) = split_host(host_header)?;
        facts.routes.iter().find(|route| {
            !route.funnel
                && route.host_port().is_some_and(|(name, route_port)| {
                    name == host && port.unwrap_or(default_port(route)) == route_port
                })
        })
    }
}

fn default_port(route: &ServeRoute) -> u16 {
    if route.is_https() { 443 } else { 80 }
}

/// A `Host` header as a lowercased name (one trailing dot dropped) and the port
/// it names, if any. `None` for an IP literal or a malformed value: a serve
/// route is always a name.
fn split_host(header: &str) -> Option<(String, Option<u16>)> {
    let header = header.trim();
    if header.starts_with('[') {
        return None;
    }
    let (name, port) = match header.rsplit_once(':') {
        Some((name, port)) => (name, Some(port.parse::<u16>().ok()?)),
        None => (header, None),
    };
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    if name.is_empty() || name.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    Some((name, port))
}

/// The exposure a serve's loop writes and everything else reads. Cloning
/// shares it.
#[derive(Clone, Debug)]
pub struct ExposureCell(Arc<tokio::sync::watch::Sender<Exposure>>);

impl ExposureCell {
    /// A cell starting at `funnel` with no identity facts.
    pub fn new(funnel: FunnelState) -> Self {
        Self(Arc::new(tokio::sync::watch::Sender::new(Exposure {
            funnel,
            identity: None,
        })))
    }

    /// The exposure right now.
    pub fn get(&self) -> Exposure {
        self.0.borrow().clone()
    }

    /// The Funnel state right now.
    pub fn funnel(&self) -> FunnelState {
        self.0.borrow().funnel
    }

    /// Set the Funnel state, answering with the one it replaced.
    pub fn set_funnel(&self, funnel: FunnelState) -> FunnelState {
        let mut before = funnel;
        self.0.send_if_modified(|exposure| {
            before = exposure.funnel;
            exposure.funnel = funnel;
            before != funnel
        });
        before
    }

    /// Hold the facts of the current look, or forget them.
    pub fn set_identity(&self, identity: Option<IdentityFacts>) {
        self.0.send_if_modified(|exposure| {
            let changed = exposure.identity != identity;
            exposure.identity = identity;
            changed
        });
    }

    /// A receiver that wakes whenever the exposure changes.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Exposure> {
        self.0.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(url: &str, funnel: bool) -> ServeRoute {
        ServeRoute {
            url: url.to_string(),
            funnel,
        }
    }

    fn with_routes(routes: Vec<ServeRoute>) -> Exposure {
        Exposure {
            funnel: FunnelState::Open,
            identity: Some(IdentityFacts {
                routes,
                ..IdentityFacts::default()
            }),
        }
    }

    #[test]
    fn loopback_is_trusted_only_while_nothing_can_reach_it_from_elsewhere() {
        assert!(!Exposure::default().loopback_possibly_public());
        for state in [
            FunnelState::Checking,
            FunnelState::Unconfirmed,
            FunnelState::CliNotFound,
            FunnelState::Funnel,
            FunnelState::FunnelSaved,
        ] {
            let exposure = Exposure {
                funnel: state,
                identity: None,
            };
            assert!(exposure.loopback_possibly_public(), "{state:?}");
        }
        let forward = Exposure {
            funnel: FunnelState::Open,
            identity: Some(IdentityFacts {
                forward_to_dux: true,
                ..IdentityFacts::default()
            }),
        };
        assert!(forward.loopback_possibly_public());
        assert!(forward.known_published());
        let checking = Exposure {
            funnel: FunnelState::Checking,
            identity: None,
        };
        assert!(
            !checking.known_published(),
            "unknown is not a fact to warn about"
        );
    }

    #[test]
    fn a_route_is_confirmed_by_its_name_and_port_and_never_through_funnel() {
        let exposure = with_routes(vec![
            route("https://box.tail.ts.net", false),
            route("https://box.tail.ts.net:8443", true),
            route("http://box.tail.ts.net:8080", false),
        ]);
        assert!(exposure.confirmed_route("box.tail.ts.net").is_some());
        assert!(exposure.confirmed_route("BOX.tail.ts.net.:443").is_some());
        assert!(
            exposure.confirmed_route("box.tail.ts.net:8443").is_none(),
            "a Funnel route is the internet, not the tailnet"
        );
        assert!(exposure.confirmed_route("box.tail.ts.net:8080").is_some());
        assert!(exposure.confirmed_route("box.tail.ts.net:9999").is_none());
        assert!(exposure.confirmed_route("other.tail.ts.net").is_none());
        assert!(exposure.confirmed_route("127.0.0.1").is_none());
        assert!(exposure.confirmed_route("[::1]:443").is_none());
        assert!(
            Exposure::default()
                .confirmed_route("box.tail.ts.net")
                .is_none(),
            "no look, no route"
        );
    }

    #[tokio::test]
    async fn the_cell_wakes_its_readers_only_on_a_change() {
        let cell = ExposureCell::new(FunnelState::Checking);
        let mut rx = cell.subscribe();
        rx.borrow_and_update();
        assert_eq!(cell.set_funnel(FunnelState::Open), FunnelState::Checking);
        assert!(rx.has_changed().unwrap());
        rx.borrow_and_update();
        assert_eq!(cell.set_funnel(FunnelState::Open), FunnelState::Open);
        assert!(!rx.has_changed().unwrap(), "the same state is no news");
        cell.set_identity(Some(IdentityFacts::default()));
        assert!(rx.has_changed().unwrap());
        rx.borrow_and_update();
        cell.set_identity(Some(IdentityFacts::default()));
        assert!(!rx.has_changed().unwrap());
    }
}
