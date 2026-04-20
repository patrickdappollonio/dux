//! Host-side remote-share session driver.
//!
//! Bind an endpoint, advertise a pairing code, accept the first valid
//! client connection, complete the HKDF-PIN handshake, then pump capture
//! events out as `RemoteMessage::FrameDiff` / `ScreenCleared` / `Resize`
//! messages.
//!
//! MVP scope: single peer, view-only (input handling lands in Phase 7).
//! The session owns the `Endpoint` for its lifetime and drops it on exit,
//! which closes all iroh relay + QUIC state cleanly.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use crossterm::event::KeyEvent;
use iroh::Endpoint;
use tokio::sync::mpsc::{Receiver, UnboundedSender};

use super::PROTOCOL_VERSION;
use super::codec::{decode, encode};
use super::endpoint::{bind_host_endpoint, encode_endpoint_addr};
use super::handshake::host_handshake;
use super::iroh_transport::IrohTransport;
use super::messages::RemoteMessage;
use super::pairing::{PairingCodePayload, PairingSecret, encode_pairing_code};
use super::tee_backend::CaptureEvent;
use super::transport::Transport;

/// Output of `prepare_host_session`: the bound endpoint and the user-
/// facing pairing code (host keeps the endpoint alive).
pub struct HostPrepared {
    pub endpoint: Endpoint,
    pub secret: PairingSecret,
    pub code: String,
}

/// Phase-1: bind the iroh endpoint, generate a pairing secret, encode the
/// pairing code. The caller shows the code to the user, then hands the
/// `HostPrepared` back into [`serve_host_session`] to accept a client.
pub async fn prepare_host_session(
    ttl: Duration,
    relay_url: Option<String>,
) -> Result<HostPrepared> {
    let endpoint = bind_host_endpoint(relay_url.as_deref()).await?;
    // Give the endpoint a moment to come online so its relay address is
    // part of the pairing code. `online()` is preferred per iroh's docs.
    let _ = tokio::time::timeout(Duration::from_secs(5), endpoint.online()).await;
    let addr = endpoint.addr();
    let secret = PairingSecret::generate(ttl);
    let payload = PairingCodePayload {
        endpoint_addr: encode_endpoint_addr(&addr)?,
        pin: secret.pin,
    };
    let code = encode_pairing_code(&payload)?;
    Ok(HostPrepared {
        endpoint,
        secret,
        code,
    })
}

/// Events flowing from the network into the host application. Translated
/// into the app's `WorkerEvent::Remote*` variants by the caller (typically
/// `main::run_remote_share`) so this module doesn't depend on app types.
#[derive(Debug)]
pub enum RemoteInboundEvent {
    Paired { peer: String },
    Disconnected { reason: String },
    InputKey(KeyEvent),
    LeadRequested,
}

/// Accept exactly one client, handshake, then run a full-duplex session
/// until either the client disconnects or `capture_rx` closes.
///
/// Ownership: takes `endpoint` by value so the session controls its
/// lifetime; drop the endpoint on return.
pub async fn serve_host_session(
    endpoint: Endpoint,
    secret: PairingSecret,
    host_label: String,
    capture_rx: Receiver<CaptureEvent>,
    inbound_tx: UnboundedSender<RemoteInboundEvent>,
) -> Result<SessionOutcome> {
    if secret.is_expired() {
        return Ok(SessionOutcome::CodeExpired);
    }

    // Enforce the TTL across the accept + handshake wait, not just at
    // session-start. A client that dials after the code has expired would
    // otherwise be accepted and complete a handshake against an
    // effectively-invalidated secret. `timeout` races `accept` against a
    // sleep for the remaining TTL; if the sleep wins, the session ends
    // with `CodeExpired` and no client is paired.
    let remaining = Duration::from_secs(secret.seconds_remaining());
    if remaining.is_zero() {
        return Ok(SessionOutcome::CodeExpired);
    }
    let incoming_result = tokio::time::timeout(remaining, endpoint.accept()).await;
    let incoming = match incoming_result {
        Ok(Some(incoming)) => incoming,
        Ok(None) => return Err(anyhow!("endpoint closed before any client arrived")),
        Err(_) => {
            crate::logger::info("remote: pairing code expired before any client connected");
            return Ok(SessionOutcome::CodeExpired);
        }
    };

    // The handshake itself must also complete before the TTL elapses. The
    // client supplies its proof only after reading the host's nonce, so
    // without this bound a stalled client could hold the endpoint past
    // the advertised expiry.
    let remaining_post_accept = Duration::from_secs(secret.seconds_remaining());
    if remaining_post_accept.is_zero() {
        return Ok(SessionOutcome::CodeExpired);
    }

    let conn = tokio::time::timeout(remaining_post_accept, incoming)
        .await
        .map_err(|_| anyhow!("iroh: connection handshake timed out"))?
        .context("iroh: accept connection")?;

    let mut transport = IrohTransport::accept(conn).await?;
    let handshake_remaining = Duration::from_secs(secret.seconds_remaining());
    if handshake_remaining.is_zero() {
        return Ok(SessionOutcome::CodeExpired);
    }
    let peer = tokio::time::timeout(
        handshake_remaining,
        host_handshake(&mut transport, &secret.pin, &host_label),
    )
    .await
    .map_err(|_| anyhow!("remote: handshake timed out"))?
    .context("remote: handshake with client failed")?;
    crate::logger::info(&format!(
        "remote: peer '{peer}' paired (protocol v{PROTOCOL_VERSION})"
    ));

    // Announce the pairing to the app so it can update status + leader.
    let _ = inbound_tx.send(RemoteInboundEvent::Paired { peer: peer.clone() });

    let outcome =
        run_bidi_session(&mut transport, peer.clone(), capture_rx, inbound_tx.clone()).await?;

    let reason = match &outcome {
        SessionOutcome::CodeExpired => "code expired".to_string(),
        SessionOutcome::PeerDisconnected { .. } => "peer disconnected".to_string(),
        SessionOutcome::HostShutdown { .. } => "host shutdown".to_string(),
    };
    let _ = inbound_tx.send(RemoteInboundEvent::Disconnected { reason });
    Ok(outcome)
}

/// Full-duplex driver: outbound capture events become `FrameDiff` messages,
/// inbound wire messages (esp. `InputKey`) are translated into
/// `RemoteInboundEvent` and pumped to the app.
pub async fn run_bidi_session<T: Transport>(
    transport: &mut T,
    peer: String,
    mut capture_rx: Receiver<CaptureEvent>,
    inbound_tx: UnboundedSender<RemoteInboundEvent>,
) -> Result<SessionOutcome> {
    let mut seq: u64 = 0;
    loop {
        tokio::select! {
            event = capture_rx.recv() => {
                let Some(event) = event else {
                    // Capture channel closed — app shutting down.
                    // Encode the Bye explicitly rather than via
                    // `unwrap_or_default()`: sending an empty payload
                    // (zero-length body) would reach the peer as a
                    // successful decode of whatever bincode thinks the
                    // default RemoteMessage layout is, which is either a
                    // panic or a garbage frame depending on the variant
                    // order. On the infinitesimal chance encoding the
                    // Bye fails, log it and skip the send — the peer
                    // will see the connection close instead.
                    match encode(&RemoteMessage::Bye {
                        reason: super::messages::ByeReason::Graceful,
                    }) {
                        Ok(bytes) => {
                            let _ = transport.send(&bytes).await;
                        }
                        Err(err) => {
                            crate::logger::info(&format!(
                                "remote: failed to encode graceful Bye for peer '{peer}': {err:#}"
                            ));
                        }
                    }
                    let _ = transport.close().await;
                    return Ok(SessionOutcome::HostShutdown { peer });
                };
                if let Some(msg) = capture_event_to_message(event, &mut seq) {
                    let bytes = encode(&msg)?;
                    if let Err(err) = transport.send(&bytes).await {
                        crate::logger::info(&format!(
                            "remote: peer '{peer}' send failed: {err:#}"
                        ));
                        let _ = transport.close().await;
                        return Ok(SessionOutcome::PeerDisconnected { peer });
                    }
                }
            }
            inbound = transport.recv() => {
                let bytes = match inbound {
                    Ok(b) => b,
                    Err(err) => {
                        crate::logger::info(&format!(
                            "remote: peer '{peer}' recv failed: {err:#}"
                        ));
                        return Ok(SessionOutcome::PeerDisconnected { peer });
                    }
                };
                let msg: RemoteMessage = decode(&bytes)?;
                match msg {
                    RemoteMessage::InputKey { key } => {
                        let crossterm_key = dux_remote::key_translate::to_crossterm(&key);
                        let _ = inbound_tx.send(RemoteInboundEvent::InputKey(crossterm_key));
                    }
                    RemoteMessage::Input { .. } => {
                        // Raw-byte path reserved for future use.
                    }
                    RemoteMessage::LeaderRequest => {
                        let _ = inbound_tx.send(RemoteInboundEvent::LeadRequested);
                    }
                    RemoteMessage::Ping { nonce } => {
                        let _ = transport
                            .send(&encode(&RemoteMessage::Pong { nonce })?)
                            .await;
                    }
                    RemoteMessage::Bye { reason } => {
                        crate::logger::info(&format!(
                            "remote: peer '{peer}' said goodbye: {reason:?}"
                        ));
                        return Ok(SessionOutcome::PeerDisconnected { peer });
                    }
                    // Host-originated variants we shouldn't receive; skip.
                    RemoteMessage::Hello { .. }
                    | RemoteMessage::FullFrame { .. }
                    | RemoteMessage::FrameDiff { .. }
                    | RemoteMessage::PtySnapshotDiff { .. }
                    | RemoteMessage::LeaderChange { .. }
                    | RemoteMessage::LeaderResponse { .. }
                    | RemoteMessage::Resize { .. }
                    | RemoteMessage::Pong { .. } => {}
                }
            }
        }
    }
}

/// Translate one capture event into the matching wire message. Bumps
/// `seq` when a message is emitted; returns `None` for events we don't
/// yet represent on the wire (cursor position/visibility for now).
fn capture_event_to_message(event: CaptureEvent, seq: &mut u64) -> Option<RemoteMessage> {
    match event {
        CaptureEvent::Cells(cells) => {
            *seq = seq.wrapping_add(1);
            Some(RemoteMessage::FrameDiff { seq: *seq, cells })
        }
        CaptureEvent::Clear | CaptureEvent::ClearRegion => {
            *seq = seq.wrapping_add(1);
            // cols=0/rows=0 encodes "screen cleared, empty payload". The
            // client wipes its mirror and waits for the next `Cells`
            // event to repaint.
            Some(RemoteMessage::FullFrame {
                seq: *seq,
                cols: 0,
                rows: 0,
                cells: Vec::new(),
            })
        }
        CaptureEvent::Resize { cols, rows } => {
            *seq = seq.wrapping_add(1);
            Some(RemoteMessage::Resize { cols, rows })
        }
        CaptureEvent::CursorPosition { .. } | CaptureEvent::CursorVisible(_) => None,
    }
}

#[derive(Debug)]
pub enum SessionOutcome {
    CodeExpired,
    PeerDisconnected {
        #[allow(dead_code)] // read via Debug when logging session end
        peer: String,
    },
    HostShutdown {
        #[allow(dead_code)] // read via Debug when logging session end
        peer: String,
    },
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tokio::sync::mpsc::{
        UnboundedReceiver, UnboundedSender, channel as bounded_channel, unbounded_channel,
    };

    use super::*;
    use crate::remote::messages::{RemoteMessage, WireCell, WireColor};
    use crate::remote::transport::Transport;

    /// Minimal in-memory transport so we can exercise the capture→wire
    /// loop without iroh.
    struct InMemoryTransport {
        tx: UnboundedSender<Vec<u8>>,
        rx: UnboundedReceiver<Vec<u8>>,
    }

    impl Transport for InMemoryTransport {
        async fn send(&mut self, bytes: &[u8]) -> Result<()> {
            self.tx
                .send(bytes.to_vec())
                .map_err(|_| anyhow::anyhow!("peer hung up"))
        }
        async fn recv(&mut self) -> Result<Vec<u8>> {
            self.rx
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("peer hung up"))
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

    fn sample_cell(col: u16, sym: &str) -> WireCell {
        WireCell {
            row: 0,
            col,
            symbol: sym.to_string(),
            fg: WireColor::White,
            bg: WireColor::Reset,
            modifier: 0,
        }
    }

    #[tokio::test]
    async fn forwarder_emits_frame_diffs_with_increasing_seq() {
        let (mut host, mut client) = duplex();
        let (cap_tx, cap_rx) = bounded_channel::<CaptureEvent>(64);
        let (in_tx, _in_rx) = unbounded_channel::<RemoteInboundEvent>();
        cap_tx
            .try_send(CaptureEvent::Cells(vec![sample_cell(0, "a")]))
            .unwrap();
        cap_tx
            .try_send(CaptureEvent::Cells(vec![sample_cell(1, "b")]))
            .unwrap();
        drop(cap_tx); // triggers graceful shutdown after two messages

        let host_task = tokio::spawn(async move {
            run_bidi_session(&mut host, "peer-1".into(), cap_rx, in_tx).await
        });

        // First frame diff.
        let bytes = client.recv().await.unwrap();
        let msg = decode(&bytes).unwrap();
        match msg {
            RemoteMessage::FrameDiff { seq, cells } => {
                assert_eq!(seq, 1);
                assert_eq!(cells[0].symbol, "a");
            }
            other => panic!("unexpected: {other:?}"),
        }
        // Second frame diff.
        let bytes = client.recv().await.unwrap();
        let msg = decode(&bytes).unwrap();
        match msg {
            RemoteMessage::FrameDiff { seq, cells } => {
                assert_eq!(seq, 2);
                assert_eq!(cells[0].symbol, "b");
            }
            other => panic!("unexpected: {other:?}"),
        }
        // Graceful Bye.
        let bytes = client.recv().await.unwrap();
        let msg = decode(&bytes).unwrap();
        assert!(
            matches!(msg, RemoteMessage::Bye { .. }),
            "expected Bye, got {msg:?}"
        );
        let outcome = host_task.await.unwrap().unwrap();
        assert!(matches!(outcome, SessionOutcome::HostShutdown { .. }));
    }

    #[tokio::test]
    async fn forwarder_translates_clear_to_empty_full_frame() {
        let (mut host, mut client) = duplex();
        let (cap_tx, cap_rx) = bounded_channel::<CaptureEvent>(64);
        let (in_tx, _in_rx) = unbounded_channel::<RemoteInboundEvent>();
        cap_tx.try_send(CaptureEvent::Clear).unwrap();
        drop(cap_tx);

        let host_task =
            tokio::spawn(
                async move { run_bidi_session(&mut host, "peer".into(), cap_rx, in_tx).await },
            );

        let bytes = client.recv().await.unwrap();
        match decode(&bytes).unwrap() {
            RemoteMessage::FullFrame {
                cols, rows, cells, ..
            } => {
                assert_eq!(cols, 0);
                assert_eq!(rows, 0);
                assert!(cells.is_empty());
            }
            other => panic!("unexpected: {other:?}"),
        }
        let _ = client.recv().await;
        let _ = host_task.await.unwrap();
    }

    #[tokio::test]
    async fn forwarder_skips_cursor_events() {
        let (mut host, mut client) = duplex();
        let (cap_tx, cap_rx) = bounded_channel::<CaptureEvent>(64);
        let (in_tx, _in_rx) = unbounded_channel::<RemoteInboundEvent>();
        cap_tx.try_send(CaptureEvent::CursorVisible(true)).unwrap();
        cap_tx
            .try_send(CaptureEvent::CursorPosition { col: 1, row: 1 })
            .unwrap();
        // Now a real payload so the test has something to observe.
        cap_tx
            .try_send(CaptureEvent::Cells(vec![sample_cell(3, "z")]))
            .unwrap();
        drop(cap_tx);

        let host_task =
            tokio::spawn(
                async move { run_bidi_session(&mut host, "peer".into(), cap_rx, in_tx).await },
            );

        // The two cursor events must be silently dropped; first wire msg
        // should be the FrameDiff for 'z'.
        let bytes = client.recv().await.unwrap();
        match decode(&bytes).unwrap() {
            RemoteMessage::FrameDiff { cells, .. } => {
                assert_eq!(cells[0].symbol, "z");
            }
            other => panic!("unexpected: {other:?}"),
        }
        let _ = client.recv().await; // drain the Bye
        let _ = host_task.await.unwrap();
    }
}
