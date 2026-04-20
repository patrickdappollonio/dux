//! iroh `Endpoint` lifecycle helpers.
//!
//! Kept in its own module so unit tests can focus on pairing + handshake
//! without binding a real endpoint, and so later phases (headless `dux
//! serve`) can reuse the bootstrap path.

use std::str::FromStr;

use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl, endpoint::presets};

#[cfg(feature = "host")]
use super::iroh_transport::ALPN;

/// Parse an optional user-supplied relay URL into a `RelayMode`.
///
/// `None` → use the N0 preset's default relay mesh (iroh's public relays).
/// `Some(url)` → build a custom relay map pointing at that single URL.
/// Returns an error only on a malformed URL; an empty string is treated
/// as "unset" to match the config-file convention of empty-string-means-
/// `Option::None`.
fn resolve_relay_mode(relay_url: Option<&str>) -> Result<Option<RelayMode>> {
    match relay_url {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => {
            let url = RelayUrl::from_str(s.trim())
                .with_context(|| format!("invalid [remote].relay_url '{s}'"))?;
            Ok(Some(RelayMode::custom([url])))
        }
    }
}

/// Bind a new iroh endpoint configured to accept dux remote-share
/// connections.
///
/// Uses the `N0` preset (iroh's public relay mesh) unless `relay_url`
/// overrides it with a user-configured relay. The host's identity is
/// freshly generated each time; long-term identity storage is future
/// work (the "trust this device" feature deferred to v2).
///
/// Host-only: browser clients cannot accept incoming connections.
#[cfg(feature = "host")]
pub async fn bind_host_endpoint(relay_url: Option<&str>) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]);
    if let Some(mode) = resolve_relay_mode(relay_url)? {
        builder = builder.relay_mode(mode);
    }
    builder.bind().await.context("iroh: bind host endpoint")
}

/// Bind a client-side endpoint. Client doesn't accept anything; it only
/// dials. We still need an endpoint to hold the secret key + relay state.
pub async fn bind_client_endpoint(relay_url: Option<&str>) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::N0);
    if let Some(mode) = resolve_relay_mode(relay_url)? {
        builder = builder.relay_mode(mode);
    }
    builder.bind().await.context("iroh: bind client endpoint")
}

/// Serialize an `EndpointAddr` into bytes suitable for embedding in a
/// pairing code.
pub fn encode_endpoint_addr(addr: &EndpointAddr) -> Result<Vec<u8>> {
    bincode::serde::encode_to_vec(addr, bincode::config::standard())
        .map_err(|e| anyhow::anyhow!("endpoint addr encode failed: {e}"))
}

/// Inverse of `encode_endpoint_addr`.
pub fn decode_endpoint_addr(bytes: &[u8]) -> Result<EndpointAddr> {
    let (addr, _) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
        .map_err(|e| anyhow::anyhow!("endpoint addr decode failed: {e}"))?;
    Ok(addr)
}
