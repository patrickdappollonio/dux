//! Where a request comes from: the one transport-agnostic place every request
//! is classified, for the auth layer, the routes and the open sockets alike.
//!
//! Only what the NEAREST hop establishes is trusted:
//!
//! - The connection itself ([`Arrival`]): the address dux was reached at and
//!   the peer's address, as the kernel reported them for the accepted socket.
//!   A connection to a loopback address is from this machine. One to an
//!   address the CURRENT Tailscale look reported as this machine's, from a
//!   peer in Tailscale's ranges, is from the tailnet; the ranges alone never
//!   are, because carrier-grade NAT, Cloudflare WARP and cloud VPCs use them
//!   too. A connection FROM any of this machine's own addresses (the one it
//!   reached, a Tailscale one, any interface's) is the network and unverified,
//!   since a relay on this machine looks exactly like that. Anything else is
//!   the network, verified by its peer address.
//! - Tailscale's Funnel marker (`Tailscale-Funnel-Request`), which Tailscale's
//!   serve proxy sets on every request that came through Funnel and strips from
//!   client input: the internet. A client that sends it itself only makes its
//!   own request stricter.
//! - For a request that reached loopback through a forwarding proxy
//!   (`X-Forwarded-For`, `Forwarded` or `X-Real-IP` present): the tailnet only
//!   when it came through a confirmed, non-Funnel `tailscale serve` route to
//!   dux whose name and port its `Host` matches, carrying Tailscale's identity
//!   headers and no Funnel marker, whose rightmost `X-Forwarded-For` entry is a
//!   Tailscale address, and with no `X-Real-IP` or `Forwarded` naming anything
//!   else. Anything else forwarded is the network: a
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
//!
//! Classification also says which addresses a request names (the blocklist
//! applies to all of them), which one dux can VERIFY (the only kind an
//! automatic ban writes to `config.toml`), and which one an unverified forward
//! merely claims (slowed in memory, never written).

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
    /// `None` when that element is missing or does not read as an address.
    pub forwarded_client: Option<IpAddr>,
    /// Every address `X-Forwarded-For` names, in order.
    pub forwarded_for: Vec<IpAddr>,
    /// Every address `X-Real-IP` and RFC 7239 `Forwarded: for=` name.
    pub other_named: Vec<IpAddr>,
    /// Whether `X-Real-IP` or `Forwarded` carries a value that names no
    /// Tailscale address (one that does not read as an address counts).
    /// Tailscale's serve proxy never sets either to anything else.
    pub other_names_beyond_tailscale: bool,
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
        let xff: Vec<&str> = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        let forwarded_client = xff.last().and_then(|part| parse_forwarded_address(part));
        let forwarded_for = xff
            .iter()
            .filter_map(|part| parse_forwarded_address(part))
            .collect();
        let mut other_named = Vec::new();
        let mut other_names_beyond_tailscale = false;
        let mut note = |value: Option<IpAddr>| match value {
            Some(ip) => {
                if !is_tailscale(dux_core::config_auth::canonical(ip)) {
                    other_names_beyond_tailscale = true;
                }
                other_named.push(ip);
            }
            None => other_names_beyond_tailscale = true,
        };
        for value in headers.get_all("x-real-ip").iter() {
            note(
                value
                    .to_str()
                    .ok()
                    .and_then(|v| parse_forwarded_address(v.trim())),
            );
        }
        for value in headers.get_all("forwarded").iter() {
            match value.to_str() {
                Ok(value) => {
                    for named in rfc7239_for(value) {
                        note(named);
                    }
                }
                Err(_) => note(None),
            }
        }
        Self {
            arrival,
            forwarded,
            forwarded_client,
            forwarded_for,
            other_named,
            other_names_beyond_tailscale,
            host: headers
                .get(axum::http::header::HOST)
                .and_then(|h| h.to_str().ok())
                .map(str::to_string),
            identity_headers: headers.contains_key(IDENTITY_HEADER),
            funnel_marker: headers.contains_key(FUNNEL_MARKER),
        }
    }
}

/// The `for=` values of an RFC 7239 `Forwarded` header, one per element, as
/// addresses: quoted or not, an IPv6 address in brackets, either with a port.
/// An element whose `for` is not an address (`unknown`, an obfuscated name) is
/// `None`; an element with no `for` at all names nothing.
fn rfc7239_for(value: &str) -> Vec<Option<IpAddr>> {
    value
        .split(',')
        .filter_map(|element| {
            element.split(';').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                key.trim().eq_ignore_ascii_case("for").then(|| {
                    let value = value.trim().trim_matches('"');
                    parse_forwarded_address(value)
                })
            })
        })
        .collect()
}

/// An address as a forwarding header names it: a bare address, a bracketed
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
    /// Every address the request names: the peer, every `X-Forwarded-For`
    /// entry, `X-Real-IP` and every `Forwarded: for=`. The blocklist applies to
    /// all of them: a client that lies about its address can only get itself
    /// refused, and an honest proxy's real client is caught whichever header
    /// that proxy uses. dux cannot know which header a given proxy overwrites.
    pub named: Vec<IpAddr>,
    /// The client address dux can VERIFY, and so the only one an automatic ban
    /// ever writes to `blocked_addresses`: the direct peer on a listener that
    /// is not loopback, or the forwarded address of a request proven to come
    /// through `tailscale serve`. `None` for everything else.
    pub verified_ip: Option<IpAddr>,
    /// The address an unverified forwarded request claims (the rightmost
    /// `X-Forwarded-For`, else the last of `X-Real-IP` and `Forwarded`). Its
    /// failures are slowed under it in memory, and never written anywhere.
    pub claimed_ip: Option<IpAddr>,
    /// Whether the path from the browser to dux is known to be encrypted:
    /// loopback never leaves the machine, the tailnet is WireGuard end to end,
    /// and a Funnel is HTTPS.
    pub transport_encrypted: bool,
    /// Whether the request came through a confirmed `tailscale serve` route
    /// served over HTTPS, or through a Funnel: the cases dux knows the browser
    /// used HTTPS.
    pub https_serve_route: bool,
    /// Whether the request was forwarded by a proxy dux cannot vouch for, the
    /// case the one-time proxy warning is about.
    pub unvouched_proxy: bool,
    /// Why a request that reached dux from this machine (over loopback, or
    /// from this machine's own Tailscale address) was counted as the network
    /// rather than this machine or the tailnet, or `None`.
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

/// Why a connection from one of this machine's own addresses is not this
/// machine or the tailnet.
const OWN_ADDRESS: &str = "the connection came from one of this machine's own addresses, as a \
     relay on this machine (socat, `ssh -L`, a proxy, a Funnel TCP forward) does too";

/// Whether a direct connection came from this machine itself: its peer is the
/// address it reached, one of this machine's Tailscale addresses from the
/// current look, or an address on one of this machine's interfaces. The ONE
/// own-address check for every listener (decided, after review): a connection
/// this machine opens to its own non-loopback address leaves FROM one of its
/// own addresses, and dux cannot tell the owner from a relay on this machine
/// aimed there, so it is the network, never this machine; and it is never
/// written anywhere or banned, because the address is this machine's.
fn own_address(peer: IpAddr, local: IpAddr, exposure: &Exposure, interfaces: &[IpAddr]) -> bool {
    peer == local
        || exposure.own_tailscale_ip(peer)
        || interfaces
            .iter()
            .any(|own| dux_core::config_auth::canonical(*own) == peer)
}

/// What a request would be if the exposure gate were open: the only way a
/// request becomes this machine or the tailnet.
enum Candidate {
    /// A loopback connection with no forwarding header.
    ThisMachine { peer: IpAddr },
    /// A direct peer on the Tailscale listener.
    TailnetPeer { peer: IpAddr },
    /// A request proven, by everything it carries, to come through a confirmed
    /// `tailscale serve` route.
    ServeClient { client: IpAddr, https: bool },
}

/// Classify one request. See the module doc for the rules.
///
/// Every request that could be trusted is first reduced to a [`Candidate`],
/// and the exposure gate ([`Exposure::trust_gate`]) is then applied in ONE
/// place, so no branch can hand out this machine or the tailnet without
/// passing it (decided, after review: the forwarded branch once skipped it).
///
/// `interfaces` are the addresses on this machine's network interfaces, for
/// the own-address check.
pub fn classify(
    facts: &RequestFacts,
    exposure: &Exposure,
    interfaces: &[IpAddr],
) -> Classification {
    let base = untrusted_base(facts);
    let Some(arrival) = facts.arrival else {
        return base;
    };
    let candidate = match candidate(facts, exposure, interfaces, arrival, &base) {
        Ok(candidate) => candidate,
        Err(untrusted) => return untrusted,
    };
    let gate = match candidate {
        // Plain loopback is also distrusted while any Funnel reaches dux.
        Candidate::ThisMachine { .. } => exposure.loopback_distrust_reason(),
        Candidate::TailnetPeer { .. } | Candidate::ServeClient { .. } => exposure.trust_gate(),
    };
    match (candidate, gate) {
        (Candidate::ThisMachine { peer }, None) => Classification {
            class: ClientClass::ThisMachine,
            verified_ip: Some(peer),
            claimed_ip: None,
            transport_encrypted: true,
            ..base
        },
        (Candidate::TailnetPeer { peer }, None) => Classification {
            class: ClientClass::Tailnet,
            verified_ip: Some(peer),
            claimed_ip: None,
            transport_encrypted: true,
            ..base
        },
        (Candidate::ServeClient { client, https }, None) => Classification {
            class: ClientClass::Tailnet,
            verified_ip: Some(client),
            claimed_ip: None,
            transport_encrypted: true,
            https_serve_route: https,
            ..base
        },
        // Closed: the network, saying why. A loopback peer is never written
        // anywhere (it is this machine's own address); a direct Tailscale peer
        // is still the address it connected from, so it stays verified; a
        // serve client is only what the headers claimed, since whatever is
        // closing the gate may have written them.
        (Candidate::ThisMachine { .. }, Some(reason)) => Classification {
            loopback_distrusted: Some(reason),
            claimed_ip: None,
            ..base
        },
        (Candidate::TailnetPeer { peer }, Some(reason)) => Classification {
            loopback_distrusted: Some(reason),
            verified_ip: Some(peer),
            claimed_ip: None,
            ..base
        },
        (Candidate::ServeClient { .. }, Some(reason)) => Classification {
            loopback_distrusted: Some(reason),
            unvouched_proxy: true,
            ..base
        },
    }
}

/// The network, with every address the request names and the one it claims;
/// what every untrusted answer starts from.
fn untrusted_base(facts: &RequestFacts) -> Classification {
    let canonical = dux_core::config_auth::canonical;
    let mut named: Vec<IpAddr> = Vec::new();
    if let Some(arrival) = facts.arrival {
        named.push(canonical(arrival.peer.ip()));
    }
    named.extend(facts.forwarded_for.iter().copied().map(canonical));
    named.extend(facts.other_named.iter().copied().map(canonical));
    named.dedup();
    let claimed = facts
        .forwarded_client
        .or_else(|| facts.other_named.last().copied())
        .map(canonical);
    Classification {
        class: ClientClass::Network,
        named,
        verified_ip: None,
        claimed_ip: claimed,
        transport_encrypted: false,
        https_serve_route: false,
        unvouched_proxy: false,
        loopback_distrusted: None,
    }
}

/// The candidate a request is, or its final, untrusted classification.
fn candidate(
    facts: &RequestFacts,
    exposure: &Exposure,
    interfaces: &[IpAddr],
    arrival: Arrival,
    base: &Classification,
) -> Result<Candidate, Classification> {
    let canonical = dux_core::config_auth::canonical;
    let local = canonical(arrival.local.ip());
    let peer = canonical(arrival.peer.ip());
    if !local.is_loopback() || !peer.is_loopback() {
        // A direct peer on a listener that is not loopback is the client
        // itself, verified whatever headers it sends. The Funnel marker is
        // ignored here (decided, after review): tailscaled delivers Funnel
        // traffic only from loopback, and honouring the marker from a LAN
        // client turned its verified, bannable address into an unverified
        // claim that is only slowed.
        if own_address(peer, local, exposure, interfaces) {
            return Err(Classification {
                loopback_distrusted: Some(OWN_ADDRESS),
                claimed_ip: None,
                ..base.clone()
            });
        }
        // The tailnet only when the connection reached one of this machine's
        // Tailscale addresses as the CURRENT look reported them, from a peer
        // in Tailscale's ranges (decided, after review). The ranges alone
        // prove nothing: carrier-grade NAT, Cloudflare WARP and cloud VPCs
        // hand out 100.64.0.0/10 too, so with no current look (no CLI, a
        // failed look, `tailscale = "no"`) nothing is the tailnet by range.
        if exposure.own_tailscale_ip(local) && is_tailscale(peer) {
            return Ok(Candidate::TailnetPeer { peer });
        }
        return Err(Classification {
            verified_ip: Some(peer),
            claimed_ip: None,
            ..base.clone()
        });
    }
    // Tailscale terminates TLS for every Funnel, so a Funnel request reached
    // the browser over HTTPS: encrypted, and its cookie may be Secure. Only
    // Tailscale's serve proxy sets the marker (it strips client copies); a
    // client that forges it on a loopback stream only makes itself the
    // internet and loses its own cookie over plain HTTP. The marker only ever
    // makes a request stricter, and the address it names stays a claim.
    if facts.funnel_marker {
        return Err(Classification {
            class: ClientClass::Internet,
            transport_encrypted: true,
            https_serve_route: true,
            ..base.clone()
        });
    }
    if !facts.forwarded {
        return Ok(Candidate::ThisMachine { peer });
    }
    // Proven to come through `tailscale serve` only when everything
    // Tailscale's serve proxy sets is there and nothing it never sets is: the
    // route's own name and port in the Host, the identity headers, the
    // nearest hop's X-Forwarded-For naming a Tailscale address (a LAN client
    // named by another proxy on this machine never is), and no X-Real-IP or
    // Forwarded naming anything else (decided: a proxy that keeps the
    // client's Host and passes its headers on is how a LAN client would
    // otherwise pose as the tailnet).
    let route = facts
        .host
        .as_deref()
        .and_then(|host| exposure.confirmed_route(host));
    let tailnet_hop = facts
        .forwarded_client
        .map(canonical)
        .filter(|ip| is_tailscale(*ip));
    match (route, tailnet_hop) {
        (Some(route), Some(client))
            if facts.identity_headers && !facts.other_names_beyond_tailscale =>
        {
            Ok(Candidate::ServeClient {
                client,
                https: route.is_https(),
            })
        }
        _ => Err(Classification {
            unvouched_proxy: true,
            ..base.clone()
        }),
    }
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
        classify(&RequestFacts::of(arrival, &headers(pairs)), exposure, &[])
    }

    fn served(url: &str, funnel: bool) -> Exposure {
        Exposure {
            funnel: FunnelState::Open,
            identity: Some(IdentityFacts {
                routes: vec![ServeRoute {
                    url: url.to_string(),
                    funnel,
                }],
                own_ips: vec![OWN_TAILSCALE.parse().unwrap()],
                ..IdentityFacts::default()
            }),
        }
    }

    /// This machine's Tailscale address in the looks below.
    const OWN_TAILSCALE: &str = "100.101.102.103";

    /// A successful look that found nothing but this machine's address.
    fn looked() -> Exposure {
        Exposure {
            funnel: FunnelState::Open,
            identity: Some(IdentityFacts {
                own_ips: vec![OWN_TAILSCALE.parse().unwrap()],
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
            &looked(),
        );
        assert_eq!(c.class, ClientClass::Tailnet);
        assert!(c.transport_encrypted);
        assert_eq!(c.verified_ip, Some("100.101.102.104".parse().unwrap()));
        let spoofed = class_of(
            arrival("192.168.1.9:5", "100.101.102.103:3890"),
            &[],
            &looked(),
        );
        assert_eq!(spoofed.class, ClientClass::Network);
        let lan = class_of(arrival("192.168.1.9:5", "192.168.1.2:3890"), &[], &looked());
        assert_eq!(lan.class, ClientClass::Network);
        assert!(!lan.transport_encrypted);
        // A tailnet address claimed by a LAN client through a wildcard listener.
        let wildcard_lan = class_of(arrival("100.64.0.9:5", "192.168.1.2:3890"), &[], &looked());
        assert_eq!(wildcard_lan.class, ClientClass::Network);
    }

    #[test]
    fn the_tailscale_ranges_alone_never_make_the_tailnet() {
        // No current look: nothing is the tailnet by range.
        let no_look = class_of(
            arrival("100.101.102.104:5", "100.101.102.103:3890"),
            &[],
            &Exposure::default(),
        );
        assert_eq!(no_look.class, ClientClass::Network);
        assert!(!no_look.transport_encrypted);
        assert_eq!(
            no_look.verified_ip,
            Some("100.101.102.104".parse().unwrap()),
            "still the address it connected from"
        );
        // A carrier-grade NAT interface that is not this machine's Tailscale
        // address, with a current look.
        let cgnat = class_of(arrival("100.72.9.9:5", "100.64.20.5:3890"), &[], &looked());
        assert_eq!(cgnat.class, ClientClass::Network);
    }

    #[test]
    fn a_peer_on_any_of_this_machines_own_addresses_is_the_network_and_unverified() {
        let interfaces: Vec<IpAddr> = vec!["192.168.1.2".parse().unwrap()];
        for (peer, local) in [
            ("192.168.1.2:5", "192.168.1.2:3890"),
            ("192.168.1.2:5", "100.101.102.103:3890"),
            ("192.168.1.2:5", "10.0.0.4:3890"),
        ] {
            let c = classify(
                &RequestFacts::of(arrival(peer, local), &HeaderMap::new()),
                &looked(),
                &interfaces,
            );
            assert_eq!(c.class, ClientClass::Network, "{peer} -> {local}");
            assert_eq!(c.verified_ip, None, "never banned: {peer} -> {local}");
            assert_eq!(c.claimed_ip, None);
            assert!(c.loopback_distrusted.is_some());
        }
    }

    #[test]
    fn the_funnel_marker_is_the_internet_on_loopback_and_ignored_from_a_direct_peer() {
        let marked = [
            ("tailscale-funnel-request", "?1"),
            ("tailscale-user-login", "x@example.com"),
        ];
        let exposure = served("https://box.tail.ts.net", false);
        let c = class_of(arrival("127.0.0.1:1", LOOPBACK), &marked, &exposure);
        assert_eq!(c.class, ClientClass::Internet);
        assert!(c.transport_encrypted, "Funnel is always HTTPS");
        assert!(c.https_serve_route);
        assert_eq!(c.verified_ip, None);

        let tailnet = class_of(
            arrival("100.64.0.9:1", "100.101.102.103:3890"),
            &marked,
            &exposure,
        );
        assert_eq!(tailnet.class, ClientClass::Tailnet);
        let lan = class_of(
            arrival("198.51.100.9:1", "192.168.1.2:3890"),
            &marked,
            &exposure,
        );
        assert_eq!(lan.class, ClientClass::Network);
        assert_eq!(
            lan.verified_ip,
            Some("198.51.100.9".parse().unwrap()),
            "a direct peer stays verified, and so bannable, whatever it sends"
        );
    }

    #[test]
    fn no_trusted_class_passes_a_closed_exposure_gate() {
        let serve_headers = [
            ("host", "box.tail.ts.net"),
            ("x-forwarded-for", "100.64.0.9"),
            ("tailscale-user-login", "owner@example.com"),
        ];
        for (funnel, forward) in [
            (FunnelState::Open, true),
            (FunnelState::Checking, false),
            (FunnelState::Unconfirmed, false),
            (FunnelState::CliNotFound, false),
            (FunnelState::Unchecked, false),
        ] {
            let exposure = Exposure {
                funnel,
                identity: Some(IdentityFacts {
                    forward_to_dux: forward,
                    routes: vec![ServeRoute {
                        url: "https://box.tail.ts.net".to_string(),
                        funnel: false,
                    }],
                    ..IdentityFacts::default()
                }),
            };
            let what = format!("{funnel:?} forward={forward}");
            let serve = class_of(arrival("127.0.0.1:1", LOOPBACK), &serve_headers, &exposure);
            assert_eq!(serve.class, ClientClass::Network, "{what}");
            assert_eq!(serve.verified_ip, None, "a claim, never banned: {what}");
            assert_eq!(serve.claimed_ip, Some("100.64.0.9".parse().unwrap()));
            assert!(serve.loopback_distrusted.is_some(), "{what}");
            let plain = class_of(arrival("127.0.0.1:1", LOOPBACK), &[], &exposure);
            assert_eq!(plain.class, ClientClass::Network, "{what}");
            let peer = class_of(
                arrival("100.64.0.9:1", "100.101.102.103:3890"),
                &[],
                &exposure,
            );
            assert_eq!(peer.class, ClientClass::Network, "{what}");
        }
        // A Funnel over HTTP marks its requests, so a serve route still works.
        let funnel = Exposure {
            funnel: FunnelState::Funnel,
            ..served("https://box.tail.ts.net", false)
        };
        let serve = class_of(arrival("127.0.0.1:1", LOOPBACK), &serve_headers, &funnel);
        assert_eq!(serve.class, ClientClass::Tailnet);
        let plain = class_of(arrival("127.0.0.1:1", LOOPBACK), &[], &funnel);
        assert_eq!(
            plain.class,
            ClientClass::Network,
            "plain loopback still is not"
        );
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
        assert_eq!(c.verified_ip, Some("100.64.0.9".parse().unwrap()));

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
        assert_eq!(c.claimed_ip, Some("198.51.100.7".parse().unwrap()));
        assert_eq!(c.verified_ip, None, "a claim is not verified");
        // Duplicate headers: the last element of the last one.
        let dup = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &[
                ("x-forwarded-for", "10.0.0.1"),
                ("x-forwarded-for", "198.51.100.8"),
            ],
            &Exposure::default(),
        );
        assert_eq!(dup.claimed_ip, Some("198.51.100.8".parse().unwrap()));
        // A header from a client that reached a non-loopback listener directly is
        // not a proxy's: the peer is the client.
        let direct = class_of(
            arrival("198.51.100.9:1", "192.168.1.2:3890"),
            &[("x-forwarded-for", "127.0.0.1")],
            &Exposure::default(),
        );
        assert_eq!(direct.class, ClientClass::Network);
        assert_eq!(direct.verified_ip, Some("198.51.100.9".parse().unwrap()));
        // Unreadable: no address to count against.
        let junk = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &[("x-forwarded-for", "not-an-address")],
            &Exposure::default(),
        );
        assert_eq!(junk.class, ClientClass::Network);
        assert_eq!(junk.claimed_ip, None);
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
            assert_eq!(c.claimed_ip, Some(want.parse().unwrap()), "{value}");
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
        assert_eq!(c.claimed_ip, None);
        assert_eq!(c.verified_ip, None);
    }

    #[test]
    fn this_machine_own_tailscale_address_is_the_network() {
        let to_self = class_of(
            arrival("100.101.102.103:5", "100.101.102.103:3890"),
            &[],
            &Exposure::default(),
        );
        assert_eq!(to_self.class, ClientClass::Network);
        assert!(to_self.loopback_distrusted.is_some());
        assert_eq!(
            to_self.verified_ip, None,
            "never blocked: it is this machine"
        );
        // Another of this machine's own addresses (its IPv4 reaching its IPv6).
        let own = Exposure {
            funnel: FunnelState::Open,
            identity: Some(IdentityFacts {
                own_ips: vec![
                    "100.101.102.103".parse().unwrap(),
                    "fd7a:115c:a1e0::1".parse().unwrap(),
                ],
                ..IdentityFacts::default()
            }),
        };
        let other_own = class_of(
            arrival("[fd7a:115c:a1e0::1]:5", "[fd7a:115c:a1e0::9]:3890"),
            &[],
            &own,
        );
        assert_eq!(other_own.class, ClientClass::Network);
        let peer = class_of(
            arrival("100.101.102.104:5", "100.101.102.103:3890"),
            &[],
            &own,
        );
        assert_eq!(peer.class, ClientClass::Tailnet);
    }

    #[test]
    fn a_known_forward_distrusts_the_tailscale_listener() {
        let forward = Exposure {
            funnel: FunnelState::Open,
            identity: Some(IdentityFacts {
                forward_to_dux: true,
                own_ips: vec![OWN_TAILSCALE.parse().unwrap()],
                ..IdentityFacts::default()
            }),
        };
        let c = class_of(
            arrival("100.101.102.104:5", "100.101.102.103:3890"),
            &[],
            &forward,
        );
        assert_eq!(c.class, ClientClass::Network);
        assert!(c.loopback_distrusted.unwrap().contains("TCP forward"));
    }

    #[test]
    fn serve_is_proven_only_when_the_nearest_hop_names_a_tailscale_address() {
        let exposure = served("https://box.tail.ts.net", false);
        let with = |extra: &[(&'static str, &'static str)]| {
            let mut headers = vec![
                ("host", "box.tail.ts.net"),
                ("tailscale-user-login", "owner@example.com"),
            ];
            headers.extend_from_slice(extra);
            class_of(arrival("127.0.0.1:1", LOOPBACK), &headers, &exposure)
        };
        assert_eq!(
            with(&[("x-forwarded-for", "192.168.1.9")]).class,
            ClientClass::Network,
            "a LAN address is never what tailscale serve reports"
        );
        assert_eq!(
            with(&[("x-forwarded-for", "192.168.1.9, 100.64.0.9")]).class,
            ClientClass::Tailnet,
            "only the nearest hop decides"
        );
        assert_eq!(
            with(&[
                ("x-forwarded-for", "100.64.0.9"),
                ("x-real-ip", "192.168.1.9")
            ])
            .class,
            ClientClass::Network
        );
        assert_eq!(
            with(&[
                ("x-forwarded-for", "100.64.0.9"),
                ("forwarded", "for=\"[2001:db8::1]:443\"")
            ])
            .class,
            ClientClass::Network
        );
        assert_eq!(
            with(&[
                ("x-forwarded-for", "100.64.0.9"),
                ("forwarded", "for=unknown")
            ])
            .class,
            ClientClass::Network,
            "a name that is not an address fails closed"
        );
        assert_eq!(
            with(&[
                ("x-forwarded-for", "100.64.0.9"),
                ("x-real-ip", "100.64.0.9")
            ])
            .class,
            ClientClass::Tailnet
        );
    }

    #[test]
    fn every_address_a_request_names_is_collected() {
        let c = class_of(
            arrival("127.0.0.1:1", LOOPBACK),
            &[
                ("x-forwarded-for", "198.51.100.1, 198.51.100.2"),
                ("x-real-ip", "198.51.100.3"),
                (
                    "forwarded",
                    "for=198.51.100.4;proto=https, For=\"[2001:db8::5]:8443\", for=\"198.51.100.6:80\"",
                ),
            ],
            &Exposure::default(),
        );
        let named: Vec<String> = c.named.iter().map(ToString::to_string).collect();
        assert_eq!(
            named,
            [
                "127.0.0.1",
                "198.51.100.1",
                "198.51.100.2",
                "198.51.100.3",
                "198.51.100.4",
                "2001:db8::5",
                "198.51.100.6"
            ]
        );
        assert_eq!(c.claimed_ip, Some("198.51.100.2".parse().unwrap()));
    }
}
