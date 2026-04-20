//! Handshake executed immediately after a `Transport` connects.
//!
//! Flow (both sides drive one round trip):
//!
//! ```text
//! HOST                                 CLIENT
//!   │                                    │
//!   │ ───  ServerHello{nonce, proto} ──▶ │
//!   │                                    │   computes tag = HKDF(pin, nonce)
//!   │ ◀── ClientAuth{tag, label}  ────── │
//!   │ verifies tag == HKDF(pin, nonce)   │
//!   │ ───  ServerAccept / ServerReject ▶ │
//! ```
//!
//! The exchange is three messages long so both peers can verify each
//! other's understanding of the protocol version before trusting the
//! other's HKDF output. Everything here is transport-agnostic: the only
//! dependency is a `Transport` pipe and some `RemoteMessage` encode/decode.

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use super::PROTOCOL_VERSION;
use super::pairing::{NONCE_LEN, PIN_LEN, TAG_LEN, ct_eq, derive_proof_tag, generate_nonce};
use super::transport::Transport;

/// First message the host sends after the QUIC stream opens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerHello {
    pub protocol_version: u16,
    pub nonce: [u8; NONCE_LEN],
}

/// Client's reply: proof of knowledge of the PIN, plus a short label the
/// host can show in its status chip ("laptop-kitchen", "work-desk", …).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientAuth {
    pub protocol_version: u16,
    pub tag: [u8; TAG_LEN],
    pub peer_label: String,
}

/// Host's final decision, sent once per connection attempt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerFinal {
    Accept { peer_label: String },
    Reject { reason: RejectReason },
}

/// Why the host rejected a handshake. Communicated so the client can show
/// the user a specific error instead of "connection closed".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RejectReason {
    ProtocolMismatch { host: u16, client: u16 },
    AuthFailed,
    CodeExpired,
    CodeAlreadyUsed,
    RateLimited,
    InternalError(String),
}

fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    bincode::serde::encode_to_vec(v, bincode::config::standard())
        .map_err(|e| anyhow!("handshake encode failed: {e}"))
}

fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<T> {
    let (v, _) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
        .map_err(|e| anyhow!("handshake decode failed: {e}"))?;
    Ok(v)
}

/// Host-side handshake driver. The caller provides the PIN expected for
/// this pairing code and a `peer_label` for itself. Returns the client's
/// self-reported label on success.
pub async fn host_handshake<T: Transport>(
    transport: &mut T,
    pin: &[u8; PIN_LEN],
    host_label: &str,
) -> Result<String> {
    let nonce = generate_nonce();
    let hello = ServerHello {
        protocol_version: PROTOCOL_VERSION,
        nonce,
    };
    transport.send(&encode(&hello)?).await?;

    let auth_bytes = transport.recv().await?;
    let auth: ClientAuth = decode(&auth_bytes)?;

    if auth.protocol_version != PROTOCOL_VERSION {
        let reject = ServerFinal::Reject {
            reason: RejectReason::ProtocolMismatch {
                host: PROTOCOL_VERSION,
                client: auth.protocol_version,
            },
        };
        let _ = transport.send(&encode(&reject)?).await;
        bail!(
            "remote: client speaks protocol v{}, we speak v{PROTOCOL_VERSION}",
            auth.protocol_version
        );
    }

    let expected = derive_proof_tag(pin, &nonce);
    if !ct_eq(&expected, &auth.tag) {
        let reject = ServerFinal::Reject {
            reason: RejectReason::AuthFailed,
        };
        let _ = transport.send(&encode(&reject)?).await;
        bail!("remote: client supplied an invalid PIN proof");
    }

    let accept = ServerFinal::Accept {
        peer_label: host_label.to_string(),
    };
    transport.send(&encode(&accept)?).await?;
    Ok(auth.peer_label)
}

/// Client-side handshake driver. Returns the host's self-reported label on
/// success. The PIN must have been extracted from the pairing code.
pub async fn client_handshake<T: Transport>(
    transport: &mut T,
    pin: &[u8; PIN_LEN],
    client_label: &str,
) -> Result<String> {
    let hello_bytes = transport.recv().await?;
    let hello: ServerHello = decode(&hello_bytes)?;

    if hello.protocol_version != PROTOCOL_VERSION {
        // Let the host learn what we speak before bailing.
        let auth = ClientAuth {
            protocol_version: PROTOCOL_VERSION,
            tag: [0u8; TAG_LEN],
            peer_label: client_label.to_string(),
        };
        let _ = transport.send(&encode(&auth)?).await;
        bail!(
            "remote: host speaks protocol v{}, we speak v{PROTOCOL_VERSION}",
            hello.protocol_version
        );
    }

    let tag = derive_proof_tag(pin, &hello.nonce);
    let auth = ClientAuth {
        protocol_version: PROTOCOL_VERSION,
        tag,
        peer_label: client_label.to_string(),
    };
    transport.send(&encode(&auth)?).await?;

    let final_bytes = transport.recv().await?;
    let decision: ServerFinal = decode(&final_bytes)?;
    match decision {
        ServerFinal::Accept { peer_label } => Ok(peer_label),
        ServerFinal::Reject { reason } => {
            bail!("remote: host rejected handshake: {reason:?}")
        }
    }
}

#[cfg(all(test, feature = "host"))]
mod tests {
    use anyhow::Result;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    use super::*;
    use crate::pairing::PIN_LEN;

    /// Paired in-memory transport for handshake tests.
    struct InMemoryTransport {
        tx: UnboundedSender<Vec<u8>>,
        rx: UnboundedReceiver<Vec<u8>>,
    }

    impl Transport for InMemoryTransport {
        async fn send(&mut self, bytes: &[u8]) -> Result<()> {
            self.tx
                .send(bytes.to_vec())
                .map_err(|_| anyhow!("peer hung up"))
        }
        async fn recv(&mut self) -> Result<Vec<u8>> {
            self.rx.recv().await.ok_or_else(|| anyhow!("peer hung up"))
        }
        async fn close(&mut self) -> Result<()> {
            Ok(())
        }
    }

    fn duplex() -> (InMemoryTransport, InMemoryTransport) {
        let (a_tx, b_rx) = unbounded_channel::<Vec<u8>>();
        let (b_tx, a_rx) = unbounded_channel::<Vec<u8>>();
        (
            InMemoryTransport { tx: a_tx, rx: a_rx },
            InMemoryTransport { tx: b_tx, rx: b_rx },
        )
    }

    #[tokio::test]
    async fn handshake_with_matching_pin_succeeds() {
        let (mut host, mut client) = duplex();
        let pin = [0x42u8; PIN_LEN];
        let host_fut = async move {
            let label = host_handshake(&mut host, &pin, "home-desk").await?;
            anyhow::Ok(label)
        };
        let client_fut = async move {
            let label = client_handshake(&mut client, &pin, "laptop").await?;
            anyhow::Ok(label)
        };
        let (host_got, client_got) = tokio::join!(host_fut, client_fut);
        let host_got = host_got.expect("host handshake");
        let client_got = client_got.expect("client handshake");
        assert_eq!(host_got, "laptop");
        assert_eq!(client_got, "home-desk");
    }

    #[tokio::test]
    async fn handshake_with_wrong_pin_is_rejected() {
        let (mut host, mut client) = duplex();
        let host_pin = [0x11u8; PIN_LEN];
        let wrong_pin = [0x22u8; PIN_LEN];
        let host_fut = async move { host_handshake(&mut host, &host_pin, "home").await };
        let client_fut = async move { client_handshake(&mut client, &wrong_pin, "laptop").await };
        let (host_res, client_res) = tokio::join!(host_fut, client_fut);
        assert!(host_res.is_err(), "host should reject bad PIN");
        assert!(client_res.is_err(), "client should see a rejection");
        let msg = format!("{:?}", client_res.unwrap_err());
        assert!(
            msg.contains("AuthFailed"),
            "reject reason should be AuthFailed: {msg}"
        );
    }
}
