//! Where a request comes from: the one transport-agnostic place every request
//! is classified, for the auth layer, the routes and the open sockets alike.
//!
//! Only what the NEAREST hop establishes is trusted:
//!
//! - The connection itself ([`Arrival`]): the address dux was reached at and
//!   the peer's address, as the kernel reported them for the accepted socket.
//!   A connection to a loopback address is from this machine; one to this
//!   machine's Tailscale address FROM a Tailscale address is from the tailnet
//!   (only a tailnet peer can send from one there); anything else is the
//!   network.
//! - Tailscale's Funnel marker (`Tailscale-Funnel-Request`), which Tailscale's
//!   serve proxy sets on every request that came through Funnel and strips from
//!   client input: the internet. A client that sends it itself only makes its
//!   own request stricter.
//! - For a request that reached loopback through a forwarding proxy
//!   (`X-Forwarded-For`, `Forwarded` or `X-Real-IP` present): the tailnet only
//!   when it came through a confirmed, non-Funnel `tailscale serve` route to
//!   dux whose name and port its `Host` matches, carrying Tailscale's identity
//!   headers and no Funnel marker. Anything else forwarded is the network: a
//!   `100.64.0.0/10` address in a header proves nothing, and there is no
//!   general trusted-proxy setting.
//! - A loopback request with NO forwarding header is this machine, except while
//!   the serve's exposure says something could hand dux such a stream from
//!   elsewhere (a Funnel, a raw TCP forward, or not knowing yet), when it is the
//!   network.
//!
//! Client-sent `Tailscale-*` identity headers on their own never mean tailnet.
//! A proxy on this machine that adds no forwarding header at all is
//! indistinguishable from this machine; that is what `require = "everywhere"`
//! is for, and what the one-time proxy warning points at.

use std::net::{IpAddr, SocketAddr};

use axum::extract::connect_info::Connected;
use axum::http::HeaderMap;
use axum::serve::IncomingStream;

use crate::exposure::Exposure;

/// The two ends of an accepted connection, as the kernel reported them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    /// The client's address (the nearest hop).
    pub peer: SocketAddr,
    /// The address dux was reached at: the listener's own address, or for a
    /// wildcard listener the specific local address the connection used.
    pub local: SocketAddr,
}

/// A listener that records both ends of each connection it accepts, so a serve
/// built with `into_make_service_with_connect_info::<Arrival>()` hands every
/// request an [`Arrival`]. Wraps whatever listener the serve already uses
/// (every serve leg's is a TCP listener with Nagle switched off), whose own
/// accept, error handling included, is unchanged.
pub struct Recorded<L>(pub L);

impl<L> axum::serve::Listener for Recorded<L>
where
    L: axum::serve::Listener<Io = tokio::net::TcpStream, Addr = SocketAddr>,
{
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    fn accept(&mut self) -> impl std::future::Future<Output = (Self::Io, Self::Addr)> + Send {
        self.0.accept()
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

impl<L> Connected<IncomingStream<'_, Recorded<L>>> for Arrival
where
    L: axum::serve::Listener<Io = tokio::net::TcpStream, Addr = SocketAddr>,
{
    fn connect_info(stream: IncomingStream<'_, Recorded<L>>) -> Self {
        // A socket whose local address cannot be read is classed by an
        // unspecified one: neither loopback nor Tailscale, so the network.
        let local = stream
            .io()
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        Self {
            peer: *stream.remote_addr(),
            local,
        }
    }
}

/// Who a request is, as far as the password goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientClass {
    /// This machine, over loopback, with nothing that could have relayed it.
    ThisMachine,
    /// A peer on this machine's own tailnet.
    Tailnet,
    /// Anyone else: the local network, a proxy dux cannot vouch for, or a
    /// connection dux could not identify.
    Network,
    /// The public internet, through Tailscale Funnel.
    Internet,
}

impl ClientClass {
    /// The spelling the status route reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThisMachine => "this_machine",
            Self::Tailnet => "tailnet",
            Self::Network => "network",
            Self::Internet => "internet",
        }
    }
}

const FUNNEL_MARKER: &str = "tailscale-funnel-request";
const IDENTITY_HEADER: &str = "tailscale-user-login";
const FORWARDING_HEADERS: [&str; 3] = ["x-forwarded-for", "forwarded", "x-real-ip"];

/// Everything about one request that classification reads, owned so an open
/// socket can classify its client again whenever the exposure changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestFacts {
    /// `None` when the server did not record the connection (a router served
    /// without it), which classifies as the network.
    pub arrival: Option<Arrival>,
    /// Whether any forwarding header is present.
    pub forwarded: bool,
    /// The rightmost `X-Forwarded-For` address: the one the nearest proxy saw.
    pub forwarded_client: Option<IpAddr>,
    /// The `Host` header, as sent.
    pub host: Option<String>,
    /// Whether Tailscale's identity headers are present.
    pub identity_headers: bool,
    /// Whether Tailscale's Funnel marker is present.
    pub funnel_marker: bool,
}

impl RequestFacts {
    /// Read the facts of a request.
    pub fn of(arrival: Option<Arrival>, headers: &HeaderMap) -> Self {
        let forwarded = FORWARDING_HEADERS
            .iter()
            .any(|name| headers.contains_key(*name));
        // Every X-Forwarded-For header, in order: the last element of the last
        // header is what the nearest proxy appended.
        let forwarded_client = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .rfind(|part| !part.is_empty())
            .and_then(parse_forwarded_address);
        Self {
            arrival,
            forwarded,
            forwarded_client,
            host: headers
                .get(axum::http::header::HOST)
                .and_then(|h| h.to_str().ok())
                .map(str::to_string),
            identity_headers: headers.contains_key(IDENTITY_HEADER),
            funnel_marker: headers.contains_key(FUNNEL_MARKER),
        }
    }
}

/// An `X-Forwarded-For` element as an address: a bare address, a bracketed
/// IPv6 one, or either with a port.
fn parse_forwarded_address(part: &str) -> Option<IpAddr> {
    if let Ok(ip) = part.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Ok(addr) = part.parse::<SocketAddr>() {
        return Some(addr.ip());
    }
    part.strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .and_then(|(inner, _)| inner.parse().ok())
}

/// What classification concluded about a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classification {
    pub class: ClientClass,
    /// The address failures are counted against and bans apply to: the peer,
    /// or for a forwarded request the rightmost forwarded address. `None` when
    /// a forwarded request named no readable address.
    pub client_ip: Option<IpAddr>,
    /// Whether the path from the browser to dux is known to be encrypted:
    /// loopback never leaves the machine, and the tailnet is WireGuard
    /// end to end.
    pub transport_encrypted: bool,
    /// Whether the request came through a confirmed `tailscale serve` route
    /// served over HTTPS, the one case dux knows the browser used HTTPS.
    pub https_serve_route: bool,
    /// Whether the request was forwarded by a proxy dux cannot vouch for, the
    /// case the one-time proxy warning is about.
    pub unvouched_proxy: bool,
    /// Why a plain loopback request was counted as the network rather than
    /// this machine (see [`Exposure::loopback_distrust_reason`]), or `None`.
    pub loopback_distrusted: Option<&'static str>,
}

impl Classification {
    /// Whether this is this machine itself, the one client that is never
    /// blocked and never counted against the global failure limit.
    pub fn verified_this_machine(&self) -> bool {
        self.class == ClientClass::ThisMachine
    }
}

fn is_tailscale(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => dux_core::tailscale::is_tailscale_cgnat(v4),
        IpAddr::V6(v6) => dux_core::tailscale::is_tailscale_ipv6(v6),
    }
}

/// Classify one request. See the module doc for the rules.
pub fn classify(facts: &RequestFacts, exposure: &Exposure) -> Classification {
    let canonical = dux_core::config_auth::canonical;
    let Some(arrival) = facts.arrival else {
        return Classification {
            class: ClientClass::Network,
            client_ip: facts.forwarded_client.map(canonical),
            transport_encrypted: false,
            https_serve_route: false,
            unvouched_proxy: false,
            loopback_distrusted: None,
        };
    };
    let local = canonical(arrival.local.ip());
    let peer = canonical(arrival.peer.ip());
    let client_ip = if facts.forwarded && local.is_loopback() {
        facts.forwarded_client.map(canonical)
    } else {
        Some(peer)
    };
    let network = |unvouched_proxy: bool| Classification {
        class: ClientClass::Network,
        client_ip,
        transport_encrypted: false,
        https_serve_route: false,
        unvouched_proxy,
        loopback_distrusted: None,
    };
    // Tailscale terminates TLS for every Funnel, so a Funnel request reached
    // the browser over HTTPS: encrypted, and its cookie may be Secure. Only
    // Tailscale's serve proxy sets the marker (it strips client copies); a
    // client that forges it on a direct request only makes itself the
    // internet and loses its own cookie over plain HTTP.
    if facts.funnel_marker {
        return Classification {
            class: ClientClass::Internet,
            transport_encrypted: true,
            https_serve_route: true,
            ..network(false)
        };
    }
    if local.is_loopback() {
        if !facts.forwarded {
            if let Some(reason) = exposure.loopback_distrust_reason() {
                return Classification {
                    loopback_distrusted: Some(reason),
                    ..network(false)
                };
            }
            return Classification {
                class: ClientClass::ThisMachine,
                client_ip,
                transport_encrypted: true,
                https_serve_route: false,
                unvouched_proxy: false,
                loopback_distrusted: None,
            };
        }
        let route = facts
            .host
            .as_deref()
            .and_then(|host| exposure.confirmed_route(host));
        return match route {
            Some(route) if facts.identity_headers => Classification {
                class: ClientClass::Tailnet,
                client_ip,
                transport_encrypted: true,
                https_serve_route: route.is_https(),
                unvouched_proxy: false,
                loopback_distrusted: None,
            },
            _ => network(true),
        };
    }
    if is_tailscale(local) && is_tailscale(peer) {
        return Classification {
            class: ClientClass::Tailnet,
            client_ip,
            transport_encrypted: true,
            https_serve_route: false,
            unvouched_proxy: false,
            loopback_distrusted: None,
        };
    }
    network(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exposure::{FunnelState, IdentityFacts};
    use dux_core::tailscale::ServeRoute;

    fn arrival(peer: &str, local: &str) -> Option<Arrival> {
        Some(Arrival {
            peer: peer.parse().unwrap(),
            local: local.parse().unwrap(),
        })
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        map
    }

    fn class_of(
        arrival: Option<Arrival>,
        pairs: &[(&str, &str)],
        exposure: &Exposure,
    ) -> Classification {
        classify(&RequestFacts::of(arrival, &headers(pairs)), exposure)
    }

    fn served(url: &str, funnel: bool) -> Exposure {
        Exposure {
            funnel: FunnelState::Open,
            identity: Some(IdentityFacts {
                routes: vec![ServeRoute {
                    url: url.to_string(),
                    funnel,
                }],
                ..IdentityFacts::default()
            }),
        }
    }

    const LOOPBACK: &str = "127.0.0.1:3890";

    #[test]
    fn plain_loopback_is_this_machine_and_encrypted() {
        let c = class_of(
            arrival("127.0.0.1:50000", LOOPBACK),
            &[("host", "localhost")],
            &Exposure::default(),
        );
        assert_eq!(c.class, ClientClass::ThisMachine);
        assert!(c.transport_encrypted);
        assert!(c.verified_this_machine());
        let v6 = class_of(
            arrival("[::1]:50000", "[::1]:3890"),
            &[],
            &Exposure::default(),
        );
        assert_eq!(v6.class, ClientClass::ThisMachine);
        // A wildcard listener reached over loopback.
        let mapped = class_of(
            arrival("[::ffff:127.0.0.1]:5", "[::ffff:127.0.0.1]:3890"),
            &[],
            &Exposure::default(),
        );
        assert_eq!(mapped.class, ClientClass::ThisMachine);
    }

    #[test]
    fn loopback_is_the_network_while_something_could_relay_it() {
        for funnel in [
            FunnelState::Checking,
            FunnelState::Unconfirmed,
            FunnelState::CliNotFound,
            FunnelState::Funnel,
            FunnelState::FunnelSaved,
        ] {
            let exposure = Exposure {
                funnel,
                identity: None,
            };
            let c = class_of(arrival("127.0.0.1:1", LOOPBACK), &[], &exposure);
            assert_eq!(c.class, ClientClass::Network, "{funnel:?}");
            assert!(!c.verified_this_machine());
        }
        let forward = Exposure {
            funnel: FunnelState::Open,
            identity: Some(IdentityFacts {
                forward_to_dux: true,
                ..IdentityFacts::default()
            }),
        };
        assert_eq!(
            class_of(arrival("127.0.0.1:1", LOOPBACK), &[], &forward).class,
            ClientClass::Network
        );
    }

    #[test]
    fn loopback_is_the_network_with_its_reason_while_dux_does_not_check_tailscale() {
        let unchecked = Exposure {
            funnel: FunnelState::Unchecked,
            identity: None,
        };
        let c = class_of(arrival("127.0.0.1:1", LOOPBACK), &[], &unchecked);
        assert_eq!(c.class, ClientClass::Network);
        assert!(!c.transport_encrypted);
        assert!(
            c.loopback_distrusted
                .unwrap()
                .contains("cannot check for a Tailscale Funnel")
        );
        let trusted = class_of(arrival("127.0.0.1:1", LOOPBACK), &[], &Exposure::default());
        assert_eq!(trusted.loopback_distrusted, None);
    }

    #[test]
    fn the_tailscale_listener_is_the_tailnet_only_from_a_tailscale_peer() {
        let c = class_of(
            arrival("100.101.102.104:5", "100.101.102.103:3890"),
            &[],
            &Exposure::default(),
        );
        assert_eq!(c.class, ClientClass::Tailnet);
        assert!(c.transport_encrypted);
        assert_eq!(c.client_ip, Some("100.101.102.104".parse().unwrap()));
        let spoofed = class_of(
            arrival("192.168.1.9:5", "100.101.102.103:3890"),
            &[],
            &Exposure::default(),
        );
        assert_eq!(spoofed.class, ClientClass::Network);
        let lan = class_of(
            arrival("192.168.1.9:5", "192.168.1.2:3890"),
            &[],
            &Exposure::default(),
        );
        assert_eq!(lan.class, ClientClass::Network);
        assert!(!lan.transport_encrypted);
        // A tailnet address claimed by a LAN client through a wildcard listener.
        let wildcard_lan = class_of(
            arrival("100.64.0.9:5", "192.168.1.2:3890"),
            &[],
            &Exposure::default(),
        );
        assert_eq!(wildcard_lan.class, ClientClass::Network);
    }

    #[test]
    fn the_funnel_marker_is_the_internet_wherever_it_arrives() {
        for (peer, local) in [
            ("127.0.0.1:1", LOOPBACK),
            ("100.64.0.9:1", "100.101.102.103:3890"),
        ] {
            let c = class_of(
                arrival(peer, local),
                &[
                    ("tailscale-funnel-request", "?1"),
                    ("tailscale-user-login", "x@example.com"),
                ],
                &served("https://box.tail.ts.net", false),
            );
            assert_eq!(c.class, ClientClass::Internet);
            assert!(c.transport_encrypted, "Funnel is always HTTPS");
            assert!(c.https_serve_route);
        }
    }

    #[test]
    fn a_forwarded_request_is_the_network_unless_a_confirmed_serve_route_vouches_for_it() {
        let exposure = served("https://box.tail.ts.net", false);
        let through_serve = [
            ("host", "box.tail.ts.net"),
            ("x-forwarded-for", "100.64.0.9"),
            ("tailscale-user-login", "owner@example.com"),
        ];
        let c = class_of(arrival("127.0.0.1:1", LOOPBACK), &through_serve, &exposure);
        assert_eq!(c.class, ClientClass::Tailnet);
        assert!(c.transport_encrypted && c.https_serve_route);
        assert_eq!(c.client_ip, Some("100.64.0.9".parse().unwrap()));

        // No identity headers: not provably Tailscale serve.
        let bare = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &[
                ("host", "box.tail.ts.net"),
                ("x-forwarded-for", "100.64.0.9"),
            ],
            &exposure,
        );
        assert_eq!(bare.class, ClientClass::Network);
        assert!(bare.unvouched_proxy);

        // The right headers on the wrong Host (an ordinary proxy).
        let other = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &[
                ("host", "127.0.0.1:9443"),
                ("x-forwarded-for", "100.64.0.9"),
                ("tailscale-user-login", "owner@example.com"),
            ],
            &exposure,
        );
        assert_eq!(other.class, ClientClass::Network);
        assert!(!other.https_serve_route);

        // A Funnel route is never the tailnet.
        let funnelled = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &through_serve,
            &served("https://box.tail.ts.net", true),
        );
        assert_eq!(funnelled.class, ClientClass::Network);

        // No route confirmed at all.
        let unconfirmed = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &through_serve,
            &Exposure::default(),
        );
        assert_eq!(unconfirmed.class, ClientClass::Network);
    }

    #[test]
    fn a_forged_forwarding_header_never_makes_a_request_more_trusted() {
        // Claiming loopback through a proxy: the rightmost address is the one
        // the proxy saw, and the request is the network either way.
        let c = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &[("x-forwarded-for", "127.0.0.1, 198.51.100.7")],
            &Exposure::default(),
        );
        assert_eq!(c.class, ClientClass::Network);
        assert_eq!(c.client_ip, Some("198.51.100.7".parse().unwrap()));
        // Duplicate headers: the last element of the last one.
        let dup = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &[
                ("x-forwarded-for", "10.0.0.1"),
                ("x-forwarded-for", "198.51.100.8"),
            ],
            &Exposure::default(),
        );
        assert_eq!(dup.client_ip, Some("198.51.100.8".parse().unwrap()));
        // A header from a client that reached a non-loopback listener directly is
        // not a proxy's: the peer is the client.
        let direct = class_of(
            arrival("198.51.100.9:1", "192.168.1.2:3890"),
            &[("x-forwarded-for", "127.0.0.1")],
            &Exposure::default(),
        );
        assert_eq!(direct.class, ClientClass::Network);
        assert_eq!(direct.client_ip, Some("198.51.100.9".parse().unwrap()));
        // Unreadable: no address to count against.
        let junk = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &[("x-forwarded-for", "not-an-address")],
            &Exposure::default(),
        );
        assert_eq!(junk.class, ClientClass::Network);
        assert_eq!(junk.client_ip, None);
        // A port or brackets around the address.
        for (value, want) in [
            ("198.51.100.1:443", "198.51.100.1"),
            ("[2001:db8::1]:443", "2001:db8::1"),
            ("[2001:db8::2]", "2001:db8::2"),
        ] {
            let c = class_of(
                arrival("127.0.0.1:1", LOOPBACK),
                &[("x-forwarded-for", value)],
                &Exposure::default(),
            );
            assert_eq!(c.client_ip, Some(want.parse().unwrap()), "{value}");
        }
        // Tailscale identity headers alone, from anywhere, mean nothing.
        let posing = class_of(
            arrival("198.51.100.9:1", "192.168.1.2:3890"),
            &[("tailscale-user-login", "owner@example.com")],
            &served("https://box.tail.ts.net", false),
        );
        assert_eq!(posing.class, ClientClass::Network);
    }

    #[test]
    fn any_forwarding_header_marks_a_loopback_request_as_forwarded() {
        for name in ["forwarded", "x-real-ip"] {
            let c = class_of(
                arrival("127.0.0.1:1", LOOPBACK),
                &[(name, "for=198.51.100.1")],
                &Exposure::default(),
            );
            assert_eq!(c.class, ClientClass::Network, "{name}");
            assert!(c.unvouched_proxy);
        }
    }

    #[test]
    fn an_unrecorded_connection_is_the_network() {
        let c = class_of(None, &[], &Exposure::default());
        assert_eq!(c.class, ClientClass::Network);
        assert_eq!(c.client_ip, None);
    }
}
