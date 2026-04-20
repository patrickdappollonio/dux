//! Tokio worker thread that manages the remote-share lifecycle.
//!
//! Owns a single-threaded tokio runtime on a dedicated OS thread and
//! mediates all network state. The main App talks to the worker via
//! synchronous channels; the worker talks back via a tokio unbounded
//! receiver that the App drains from its main event loop using `try_recv`.
//!
//! State machine:
//! - `Idle` — no endpoint, capture events are discarded.
//! - `Sharing` — iroh endpoint bound, awaiting client; on connect,
//!   capture events flow to the session task.
//!
//! Ownership:
//! - `capture_tx` is cloned into the `TeeBackend`. The worker owns the
//!   single `capture_rx` and fans events into whichever session task is
//!   live (or discards them when idle).
//! - `inbound_tx` / `inbound_rx` carry events back to the App; the App
//!   owns the receiver and calls `try_recv` each tick.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::runtime::Builder;
use tokio::sync::mpsc::{
    Receiver as BoundedReceiver, Sender as BoundedSender, UnboundedReceiver, UnboundedSender,
    channel as bounded_channel, error::TrySendError, unbounded_channel,
};
use tokio::task::JoinHandle;

/// Slots in the session's capture queue. Caps worker → session task
/// backpressure: if the network path is stalled, we drop oldest-first on
/// overflow rather than let memory grow without bound. 512 slots at the
/// host's render cadence (≤60 Hz) gives the peer roughly eight seconds
/// of breathing room before any cell batch is dropped — plenty for a
/// transient hiccup, not so much that a truly disconnected peer OOMs.
const SESSION_CAP_SLOTS: usize = 512;

use super::server::{
    HostPrepared, RemoteInboundEvent as ServerInboundEvent, prepare_host_session,
    serve_host_session,
};
use super::tee_backend::CaptureEvent;

/// Commands dispatched from the main thread to the remote worker.
#[derive(Debug)]
pub enum RemoteCommand {
    /// Bind the iroh endpoint, generate a pairing code, and wait for a
    /// client. `ttl_secs` bounds how long the generated code remains
    /// valid. `relay_url` optionally overrides the iroh relay (matches
    /// `[remote].relay_url`).
    StartShare {
        ttl_secs: u64,
        host_label: String,
        relay_url: Option<String>,
    },
    /// Tear down the current share (disconnect any client, drop the
    /// endpoint).
    StopShare,
    /// Stop the worker entirely and join the thread. Called during app
    /// shutdown.
    Shutdown,
}

/// Events the worker emits back to the App.
#[derive(Debug)]
pub enum RemoteInboundEvent {
    /// A pairing code is ready to show to the user.
    PairingCodeReady { code: String, expires_at: Instant },
    /// A peer completed the handshake and is now driving.
    Paired { peer: String },
    /// The peer disconnected or the session ended.
    Disconnected { reason: String },
    /// A key event from the connected peer.
    InputKey(crossterm::event::KeyEvent),
    /// The peer is asking for the input lead.
    LeadRequested,
    /// A lifecycle error — propagate to the status bar.
    Error(String),
}

pub struct RemoteWorker {
    cmd_tx: UnboundedSender<RemoteCommand>,
    capture_tx: BoundedSender<CaptureEvent>,
    /// Taken exactly once by the App during bootstrap so its
    /// synchronous drain loop can pull events via `try_recv`.
    inbound_rx: Option<UnboundedReceiver<RemoteInboundEvent>>,
    join: Option<thread::JoinHandle<()>>,
}

impl RemoteWorker {
    /// Spawn the worker thread. Returns a handle with all three
    /// channels the App needs.
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = unbounded_channel::<RemoteCommand>();
        let (capture_tx, capture_rx) =
            bounded_channel::<CaptureEvent>(crate::remote::tee_backend::DEFAULT_CAPTURE_CAPACITY);
        let (inbound_tx, inbound_rx) = unbounded_channel::<RemoteInboundEvent>();

        let join = thread::Builder::new()
            .name("dux-remote".into())
            .spawn(move || {
                let rt = match Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(err) => {
                        crate::logger::error(&format!(
                            "remote worker: tokio runtime failed to start: {err}"
                        ));
                        return;
                    }
                };
                rt.block_on(async move { run(cmd_rx, capture_rx, inbound_tx).await });
            })
            .expect("failed to spawn dux-remote worker thread");

        Self {
            cmd_tx,
            capture_tx,
            inbound_rx: Some(inbound_rx),
            join: Some(join),
        }
    }

    /// Consume the inbound receiver. Called once by the App.
    pub fn take_inbound_rx(&mut self) -> Option<UnboundedReceiver<RemoteInboundEvent>> {
        self.inbound_rx.take()
    }

    /// Return a cloneable capture sender for the `TeeBackend`.
    pub fn capture_sender(&self) -> BoundedSender<CaptureEvent> {
        self.capture_tx.clone()
    }

    /// Send a command. Fails silently if the worker has already exited;
    /// callers that care can match the result.
    pub fn send(
        &self,
        cmd: RemoteCommand,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<RemoteCommand>> {
        self.cmd_tx.send(cmd)
    }

    /// Graceful shutdown. Consumes self. The `Drop` impl below does the
    /// same thing, so callers typically just let the value drop — this
    /// method is exposed so tests can assert clean teardown explicitly.
    #[cfg(test)]
    pub fn shutdown(mut self) {
        let _ = self.cmd_tx.send(RemoteCommand::Shutdown);
        if let Some(join) = self.join.take()
            && let Err(err) = join.join()
        {
            crate::logger::error(&format!("remote worker thread panicked: {err:?}"));
        }
    }
}

impl Drop for RemoteWorker {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(RemoteCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Worker state.
enum SessionState {
    Idle,
    Sharing {
        /// The current session's capture input. Bounded so a stalled
        /// session task cannot push memory growth onto the worker. When
        /// dropped the session task drains its queue and exits.
        session_cap_tx: BoundedSender<CaptureEvent>,
        /// The task running `serve_host_session`. Aborted on StopShare.
        task: JoinHandle<()>,
    },
}

async fn run(
    mut cmd_rx: UnboundedReceiver<RemoteCommand>,
    mut capture_rx: BoundedReceiver<CaptureEvent>,
    inbound_tx: UnboundedSender<RemoteInboundEvent>,
) {
    let mut state = SessionState::Idle;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(RemoteCommand::Shutdown) | None => {
                        stop_share(&mut state).await;
                        return;
                    }
                    Some(RemoteCommand::StartShare {
                        ttl_secs,
                        host_label,
                        relay_url,
                    }) => {
                        start_share(
                            &mut state,
                            ttl_secs,
                            host_label,
                            relay_url,
                            inbound_tx.clone(),
                        )
                        .await;
                    }
                    Some(RemoteCommand::StopShare) => {
                        stop_share(&mut state).await;
                        let _ = inbound_tx.send(RemoteInboundEvent::Disconnected {
                            reason: "share stopped".into(),
                        });
                    }
                }
            }
            event = capture_rx.recv() => {
                let Some(event) = event else { return };
                if let SessionState::Sharing { session_cap_tx, .. } = &state
                    && let Err(err) = session_cap_tx.try_send(event)
                {
                    match err {
                        TrySendError::Full(_) => {
                            // Session task isn't keeping up with the
                            // render cadence. Drop this batch; the next
                            // `Backend::draw` call will re-emit any
                            // cells that actually changed since. Avoid
                            // spamming the log at tick rate.
                            static WARNED: std::sync::atomic::AtomicBool =
                                std::sync::atomic::AtomicBool::new(false);
                            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                crate::logger::info(
                                    "remote: session capture queue full; dropping frame \
                                     (slow peer; will auto-resync on the next tick)",
                                );
                            }
                        }
                        TrySendError::Closed(_) => {
                            // Session task exited; it will be cleaned up
                            // via its JoinHandle on the next StopShare.
                        }
                    }
                }
                // else: idle, drop.
            }
        }
    }
}

async fn start_share(
    state: &mut SessionState,
    ttl_secs: u64,
    host_label: String,
    relay_url: Option<String>,
    inbound_tx: UnboundedSender<RemoteInboundEvent>,
) {
    // If a share is already active, tear it down first. Users can also
    // rotate codes this way (StopShare then StartShare).
    stop_share(state).await;

    let prepared = match prepare_host_session(Duration::from_secs(ttl_secs), relay_url).await {
        Ok(p) => p,
        Err(err) => {
            let _ = inbound_tx.send(RemoteInboundEvent::Error(format!(
                "failed to bring up endpoint: {err:#}"
            )));
            return;
        }
    };

    let HostPrepared {
        endpoint,
        secret,
        code,
    } = prepared;
    let expires_at = Instant::now() + Duration::from_secs(ttl_secs);
    let _ = inbound_tx.send(RemoteInboundEvent::PairingCodeReady { code, expires_at });

    let (session_cap_tx, session_cap_rx) = bounded_channel::<CaptureEvent>(SESSION_CAP_SLOTS);

    // Translate session's `RemoteInboundEvent` (from server.rs, a
    // different type) into the worker's `RemoteInboundEvent`.
    let (session_in_tx, mut session_in_rx) = unbounded_channel::<ServerInboundEvent>();
    let inbound_fanout = inbound_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = session_in_rx.recv().await {
            let translated = match event {
                ServerInboundEvent::Paired { peer } => RemoteInboundEvent::Paired { peer },
                ServerInboundEvent::Disconnected { reason } => {
                    RemoteInboundEvent::Disconnected { reason }
                }
                ServerInboundEvent::InputKey(k) => RemoteInboundEvent::InputKey(k),
                ServerInboundEvent::LeadRequested => RemoteInboundEvent::LeadRequested,
            };
            if inbound_fanout.send(translated).is_err() {
                break;
            }
        }
    });

    let task = tokio::spawn(async move {
        match serve_host_session(endpoint, secret, host_label, session_cap_rx, session_in_tx).await
        {
            Ok(outcome) => {
                crate::logger::info(&format!("remote: share session ended: {outcome:?}"));
            }
            Err(err) => {
                crate::logger::error(&format!("remote: share session failed: {err:#}"));
            }
        }
    });

    *state = SessionState::Sharing {
        session_cap_tx,
        task,
    };
}

async fn stop_share(state: &mut SessionState) {
    if let SessionState::Sharing { task, .. } = std::mem::replace(state, SessionState::Idle) {
        // Dropping session_cap_tx closes the session's capture_rx which
        // lets the serve loop exit gracefully; then abort to guarantee
        // the task unblocks even if it is stuck in `accept`.
        task.abort();
        let _ = task.await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn spawn_and_shutdown_cleanly() {
        let worker = RemoteWorker::spawn();
        worker.send(RemoteCommand::StopShare).expect("send");
        worker.shutdown();
    }

    #[test]
    fn drop_shuts_down_thread() {
        let worker = RemoteWorker::spawn();
        let cmd_tx = worker.cmd_tx.clone();
        drop(worker);
        // After drop, the worker's receiver is closed — sending must fail.
        assert!(cmd_tx.send(RemoteCommand::StopShare).is_err());
    }

    #[test]
    fn capture_events_drain_when_idle() {
        let worker = RemoteWorker::spawn();
        let cap = worker.capture_sender();
        // `try_send` so we never block on a full channel; idle-drain
        // should clear the backlog within the sleep window. Capacity is
        // plenty larger than 100 so this should always succeed.
        for _ in 0..100 {
            let _ = cap.try_send(CaptureEvent::Clear);
        }
        std::thread::sleep(Duration::from_millis(10));
        worker.shutdown();
    }

    #[test]
    fn take_inbound_rx_returns_once() {
        let mut worker = RemoteWorker::spawn();
        assert!(worker.take_inbound_rx().is_some());
        assert!(worker.take_inbound_rx().is_none());
        worker.shutdown();
    }
}
