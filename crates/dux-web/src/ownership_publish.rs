//! Turning the terminal UI's ownership facts into the broadcasts a browser's
//! own claim would have produced.
//!
//! The terminal UI takes, releases and resizes ptys while a background server
//! serves, but cannot announce any of it: both buses live on this serve's tokio
//! runtime, in a crate `dux-tui` deliberately cannot see. The facts cross the seam
//! as [`dux_core::background_serve::PtyOwnershipEvent`] data and become the same
//! two broadcasts a socket handler emits, a `pty.owner` and a grid change.
//!
//! Deliberately a dumb relay: who owns what, which epoch and which seq were all
//! decided under the owners lock before these events were built, and the epoch and
//! seq stamps are what let receivers reorder arrivals.

use std::sync::Arc;

use dux_core::background_serve::PtyOwnershipEvent;

use crate::event_bus::EventBus;
use crate::pty_sizes::PtyGridBus;

/// The two buses one serve announces ownership on, kept together so the seam has
/// a single thing to hold. Built inside `build_app`, where both buses are born,
/// and published into a slot the caller passes down: the router swallows the app
/// state whole, so there is no other way back to it.
#[derive(Clone)]
pub(crate) struct OwnershipPublisher {
    bus: Arc<EventBus>,
    grid: Arc<PtyGridBus>,
}

impl OwnershipPublisher {
    pub(crate) fn new(bus: Arc<EventBus>, grid: Arc<PtyGridBus>) -> Self {
        Self { bus, grid }
    }

    /// Announce one batch of the terminal UI's ownership facts, in batch order,
    /// which is claim order: a release published before the claim that replaced it
    /// would be discarded by the client's epoch ordering rather than obeyed.
    pub(crate) fn publish(&self, events: &[PtyOwnershipEvent]) {
        for event in events {
            match event {
                PtyOwnershipEvent::Claimed {
                    pty_id,
                    conn_id,
                    epoch,
                    device,
                } => {
                    self.bus.emit(crate::server::pty_owner_event(
                        pty_id,
                        *conn_id,
                        *epoch,
                        Some(device.as_str()),
                    ));
                }
                PtyOwnershipEvent::Released { pty_id, epoch } => {
                    self.bus
                        .emit(crate::server::pty_owner_cleared_event(pty_id, *epoch));
                }
                PtyOwnershipEvent::GridApplied {
                    pty_id,
                    rows,
                    cols,
                    seq,
                } => {
                    self.grid.publish(pty_id, *rows, *cols, *seq);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The relay must produce the SAME wire shapes a browser's own claim does,
    /// because the clients that receive them cannot tell (and must not care)
    /// which surface claimed. A regression here is silent: the browser simply
    /// never learns the terminal UI took over.
    #[tokio::test]
    async fn a_tui_claim_reaches_the_event_bus_as_an_ordinary_pty_owner_handover() {
        let bus = Arc::new(EventBus::new());
        let grid = Arc::new(PtyGridBus::default());
        let publisher = OwnershipPublisher::new(Arc::clone(&bus), Arc::clone(&grid));
        let mut events = bus.subscribe();
        let mut grids = grid.subscribe();

        publisher.publish(&[
            PtyOwnershipEvent::Claimed {
                pty_id: "s1".to_string(),
                conn_id: 7,
                epoch: 3,
                device: dux_core::background_serve::TUI_DEVICE_LABEL.to_string(),
            },
            PtyOwnershipEvent::GridApplied {
                pty_id: "s1".to_string(),
                rows: 24,
                cols: 80,
                seq: 5,
            },
            PtyOwnershipEvent::Released {
                pty_id: "s1".to_string(),
                epoch: 4,
            },
        ]);

        let crate::event_bus::Event::Resource {
            event,
            id,
            owner,
            epoch,
            device,
            ..
        } = events.recv().await.expect("the handover was emitted");
        assert_eq!(event, "pty.owner");
        assert_eq!(id.as_deref(), Some("s1"));
        assert_eq!(owner.as_deref(), Some("7"));
        assert_eq!(epoch, Some(3));
        assert_eq!(
            device.as_deref(),
            Some(dux_core::background_serve::TUI_DEVICE_LABEL),
            "a watching browser names the driving device from this field, and for \
             this participant that is the terminal UI"
        );

        let change = grids.recv().await.expect("the grid change was published");
        assert_eq!(change.pty_id, "s1");
        assert_eq!((change.rows, change.cols, change.seq), (24, 80, 5));

        let crate::event_bus::Event::Resource {
            event,
            owner,
            epoch,
            ..
        } = events.recv().await.expect("the release was emitted");
        assert_eq!(event, "pty.owner");
        assert_eq!(
            owner, None,
            "an owner-cleared handover names nobody, which is what retires the \
             watcher's take-over card"
        );
        assert_eq!(epoch, Some(4));
    }
}
