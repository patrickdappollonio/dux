//! The per-PTY grid-change broadcast: how every socket attached to one PTY
//! learns that its grid moved.
//!
//! ONE PTY HAS ONE AUTHORITATIVE GRID, the owner's. Every other attached
//! browser renders the same byte stream into its own, differently sized xterm,
//! so a viewer whose grid disagrees is rendering wrapped and clamped garbage
//! and, before this, had no way to know it: the wire never told a non-owner the
//! PTY's size. The `connected` handshake now carries the grid at attach time
//! and this bus carries every change after it, so a viewer can say so on screen
//! and heal itself with a fresh attach.
//!
//! WHY A BROADCAST CHANNEL AND NOT A REGISTRY OF SINKS. Every socket in this
//! crate is driven by its own `select!` loop and no sink is ever held anywhere
//! else (see the liveness-ping note in `server.rs` for the same reasoning
//! applied to the ping): a registry would mean holding another task's sink, and
//! locking one across an await. A `tokio::sync::broadcast` fits the existing
//! shape instead, as one more arm in the loop each socket already runs.
//!
//! WHY NOT THE EVENT BUS. `pty.owner` rides `/ws/events` because surfaces with
//! no PTY socket attached (the sidebar, the agent menu) need it. A grid change
//! is meaningful only to a client rendering that PTY's bytes, which is exactly
//! the set of sockets this bus reaches, and delivering it on the PTY socket
//! keeps it ordered against that socket's own `connected` handshake.

/// One applied grid change: the PTY whose grid moved and what it moved to.
/// Cloned per receiver, so it stays three integers and a short id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PtyGridChange {
    pub(crate) pty_id: String,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    /// The per-pty apply-order sequence stamped by `claim_for_resize` under the
    /// owners lock. Publishes happen AFTER that lock releases, so two sockets'
    /// announcements of two ordered applies can reach this bus inverted; a
    /// receiver drops any change whose seq is at or below the newest it has
    /// seen for the pty, so the stale one can never become the last word.
    pub(crate) seq: u64,
}

/// How many grid changes a slow socket may fall behind before its receiver is
/// told it lagged. A grid change is tiny and rare (one per settled resize), and
/// a lagged receiver loses nothing that matters: the NEXT change carries the
/// current geometry, and the viewer's own reconnect handshake re-reads it from
/// scratch. Sized well above any realistic burst so the lag branch is the
/// anomaly rather than the resize-drag norm.
const PTY_GRID_CHANNEL_CAPACITY: usize = 64;

/// The process-wide grid-change bus, shared by every PTY socket through
/// `AppState`.
pub(crate) struct PtyGridBus {
    tx: tokio::sync::broadcast::Sender<PtyGridChange>,
}

impl Default for PtyGridBus {
    fn default() -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel(PTY_GRID_CHANNEL_CAPACITY);
        Self { tx }
    }
}

impl PtyGridBus {
    /// Subscribe before the socket's loop starts. A receiver created after a
    /// send does not see it, which is why every PTY socket subscribes at attach
    /// and reads its own starting grid off the handshake instead.
    pub(crate) fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PtyGridChange> {
        self.tx.subscribe()
    }

    /// Announce a grid change that has ALREADY been applied. Called after the
    /// owners lock is released, like the `pty.owner` broadcast beside it, and
    /// only on the paths that really resized the child: a refused resize
    /// changed nothing and must say nothing. `seq` is the apply-order stamp
    /// the claim took under the lock (see [`PtyGridChange::seq`]). A send with
    /// no live receivers is not an error (nobody is attached).
    pub(crate) fn publish(&self, pty_id: &str, rows: u16, cols: u16, seq: u64) {
        let _ = self.tx.send(PtyGridChange {
            pty_id: pty_id.to_string(),
            rows,
            cols,
            seq,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_published_change_reaches_every_subscriber() {
        let bus = PtyGridBus::default();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.publish("s1", 30, 100, 7);
        let expected = PtyGridChange {
            pty_id: "s1".to_string(),
            rows: 30,
            cols: 100,
            seq: 7,
        };
        assert_eq!(a.recv().await.expect("subscriber a"), expected);
        assert_eq!(
            b.recv().await.expect("subscriber b"),
            expected,
            "every socket attached to the pty must hear the change, not just \
             the first one to subscribe"
        );
    }

    #[tokio::test]
    async fn publishing_with_nobody_attached_is_not_an_error() {
        // Every resize publishes, including the ones nobody is watching. A
        // `send` with no receivers returns `Err`, and treating that as a
        // failure would mean the last socket to detach breaks the next resize.
        let bus = PtyGridBus::default();
        bus.publish("s1", 24, 80, 1);
    }

    /// THE INVERSION the seq exists for: the applies serialized under the
    /// owners lock (seq order), but each socket publishes after releasing it,
    /// so the newer geometry can reach the bus FIRST and the stale one after
    /// it. A receiver keeping a last-seen high-water mark per pty forwards the
    /// newer change and drops the stale one, so the stale geometry can never
    /// become a viewer's last word.
    #[tokio::test]
    async fn a_stale_publish_arriving_after_a_newer_one_is_droppable_by_seq() {
        let bus = PtyGridBus::default();
        let mut rx = bus.subscribe();
        // B's take-over (seq 3) publishes before A's earlier apply (seq 2).
        bus.publish("s1", 30, 100, 3);
        bus.publish("s1", 24, 80, 2);

        let mut last_seen = 0u64;
        let mut forwarded = Vec::new();
        for _ in 0..2 {
            let change = rx.recv().await.expect("both publishes arrive");
            if change.seq <= last_seen {
                continue;
            }
            last_seen = change.seq;
            forwarded.push((change.rows, change.cols));
        }
        assert_eq!(
            forwarded,
            vec![(30, 100)],
            "only the newest geometry survives the filter; the stale publish \
             must not overwrite it just because it arrived last"
        );
    }
}
