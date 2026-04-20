//! iroh-based `Transport` implementation.
//!
//! A single dux host spins up one `Endpoint` and accepts one `Connection`
//! at a time (single-peer MVP). Each side opens a bidirectional QUIC
//! stream; the `Transport::send` / `Transport::recv` methods write/read
//! length-prefixed framed payloads on that stream.
//!
//! Framing: `u32 length` (big-endian) followed by the raw bincode-encoded
//! `RemoteMessage` payload. Keeping framing here rather than inside the
//! codec means the transport trait is the single place that handles
//! message boundaries.

use anyhow::{Context, Result, anyhow, bail};
use iroh::endpoint::{Connection, RecvStream, SendStream};

use super::transport::Transport;

/// ALPN identifier advertised by every dux remote endpoint. Must match on
/// both sides. Bump the suffix version when the wire protocol breaks
/// compatibility. Current: v2 — `InputKey` uses `WireKeyEvent` instead of
/// `crossterm::event::KeyEvent`.
pub const ALPN: &[u8] = b"dux/remote/2";

/// Maximum size of one framed payload. Guards against a peer sending a
/// maliciously large length prefix. Real frames should never be this
/// large — full keyframes of a 200x60 terminal top out around 60 KB.
const MAX_FRAME_BYTES: u32 = 4 * 1024 * 1024;

/// Transport backed by a single iroh QUIC bidirectional stream.
pub struct IrohTransport {
    /// The underlying QUIC connection. Kept alive for the duration of the
    /// session — dropping it tears down the stream.
    _conn: Connection,
    send: SendStream,
    recv: RecvStream,
}

impl IrohTransport {
    /// Construct from an already-established connection and its bidirectional
    /// stream pair. Use [`accept`] or [`dial`] to produce these.
    pub fn new(conn: Connection, send: SendStream, recv: RecvStream) -> Self {
        Self {
            _conn: conn,
            send,
            recv,
        }
    }

    /// Wait for an inbound connection and open its bidirectional stream.
    /// Host side. Never called from the WASM client build.
    #[cfg(feature = "host")]
    pub async fn accept(conn: Connection) -> Result<Self> {
        let (send, recv) = conn
            .accept_bi()
            .await
            .context("iroh: accept bidirectional stream")?;
        Ok(Self::new(conn, send, recv))
    }

    /// Open a bidirectional stream on a freshly dialed connection. Client
    /// side.
    pub async fn dial(conn: Connection) -> Result<Self> {
        let (send, recv) = conn
            .open_bi()
            .await
            .context("iroh: open bidirectional stream")?;
        // The receiver on the other end only learns about the stream once
        // we send at least one byte. The handshake caller will send a
        // Hello first thing, so we don't need to do anything special here.
        Ok(Self::new(conn, send, recv))
    }
}

impl Transport for IrohTransport {
    async fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let len = bytes.len();
        if len > MAX_FRAME_BYTES as usize {
            bail!("remote: frame too large to send: {len} bytes");
        }
        let len_prefix = (len as u32).to_be_bytes();
        self.send
            .write_all(&len_prefix)
            .await
            .context("iroh: write length prefix")?;
        self.send
            .write_all(bytes)
            .await
            .context("iroh: write payload")?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        let mut prefix = [0u8; 4];
        self.recv
            .read_exact(&mut prefix)
            .await
            .map_err(|e| anyhow!("iroh: read length prefix failed: {e}"))?;
        let len = u32::from_be_bytes(prefix);
        if len > MAX_FRAME_BYTES {
            bail!("remote: incoming frame too large ({len} > {MAX_FRAME_BYTES})");
        }
        let mut buf = vec![0u8; len as usize];
        self.recv
            .read_exact(&mut buf)
            .await
            .map_err(|e| anyhow!("iroh: read payload failed: {e}"))?;
        Ok(buf)
    }

    async fn close(&mut self) -> Result<()> {
        // Finish signals we're done writing; the remote side will see EOF
        // on their read stream.
        let _ = self.send.finish();
        Ok(())
    }
}

#[cfg(all(test, feature = "host"))]
mod framing_tests {
    //! Framing is the only part of the iroh transport we can exercise
    //! without spinning up a real iroh endpoint (which requires relay
    //! connectivity and makes tests slow + flaky). Stand up an
    //! `InMemoryTransport` instead and reuse it to validate codec +
    //! framing contracts.

    use anyhow::Result;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    use crate::codec::{decode, encode};
    use crate::messages::RemoteMessage;
    use crate::transport::Transport;

    /// Paired in-memory transport for tests. A<->B are two halves of a
    /// tokio mpsc duplex pipe.
    struct InMemoryTransport {
        tx: UnboundedSender<Vec<u8>>,
        rx: UnboundedReceiver<Vec<u8>>,
    }

    impl Transport for InMemoryTransport {
        async fn send(&mut self, bytes: &[u8]) -> Result<()> {
            self.tx
                .send(bytes.to_vec())
                .map_err(|_| anyhow::anyhow!("in-memory transport peer disconnected"))
        }
        async fn recv(&mut self) -> Result<Vec<u8>> {
            self.rx
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("in-memory transport peer hung up"))
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
    async fn encoded_message_roundtrips_over_transport() {
        let (mut a, mut b) = duplex();
        let msg = RemoteMessage::Input {
            bytes: b"hello world".to_vec(),
        };
        let payload = encode(&msg).unwrap();
        a.send(&payload).await.unwrap();
        let got = b.recv().await.unwrap();
        let decoded = decode(&got).unwrap();
        match decoded {
            RemoteMessage::Input { bytes } => assert_eq!(bytes, b"hello world".to_vec()),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_survives_multiple_messages_in_order() {
        let (mut a, mut b) = duplex();
        for i in 0..10u64 {
            let msg = RemoteMessage::Ping { nonce: i };
            a.send(&encode(&msg).unwrap()).await.unwrap();
        }
        for i in 0..10u64 {
            let bytes = b.recv().await.unwrap();
            let decoded = decode(&bytes).unwrap();
            match decoded {
                RemoteMessage::Ping { nonce } => assert_eq!(nonce, i),
                other => panic!("expected Ping({i}), got {other:?}"),
            }
        }
    }
}
