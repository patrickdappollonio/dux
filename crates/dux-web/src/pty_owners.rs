//! The per-PTY input-ownership registry shared by every PTY socket handler and
//! the engine actor's spine check.
//!
//! Input ownership is a WEB-layer concept: browser connections take and hand
//! over the right to type into a PTY, arbitrated here between the per-PTY
//! websockets. The engine knows nothing about it (the TUI writes through its
//! own path with no ownership gate), which is why this lives in `dux-web` and
//! why the spine's [`dux_core::viewmodel::AgentTabView::input_owner`] field is
//! filled by the web layer as an overlay rather than by the engine.
//!
//! Two consumers read it outside the socket handlers, and both are
//! deliberately narrow:
//! - the file-drop route's courtesy check
//!   ([`crate::server::AppState::input_held_by_someone_else`]), and
//! - the engine actor's spine overlay ([`Self::input_owners_snapshot`] +
//!   [`Self::ownership_generation`]), which publishes the owning connection id
//!   on the shared spine so every client — including one with no PTY socket
//!   attached — can tell that another connection is driving an agent.

/// The owner map plus the monotonic ownership epoch, guarded together by ONE std
/// Mutex so a fresh epoch is assigned in the SAME critical section that records a
/// new owner. Bumping the epoch under the lock that serializes every owner write
/// makes epochs monotonic in TRUE claim order even when two connections claim
/// concurrently, so the `pty.owner` broadcast (emitted after the lock releases, and
/// therefore freely reorderable by the runtime) can be deduped by epoch on the
/// client without confusing which claim actually won (see `ptyOwnership.ts`).
#[derive(Default)]
pub(crate) struct OwnersState {
    /// pty id -> the connection id that currently owns sizing+input.
    pub(crate) map: std::collections::HashMap<String, u64>,
    /// Bumped on every ownership CHANGE; the value handed to the caller and stamped
    /// onto the emitted `pty.owner` so clients converge on the latest claim
    /// regardless of broadcast arrival order. Never decreases within a process.
    pub(crate) epoch: u64,
    /// Bumped on every MUTATION of `map` — a claim handover, a first-writer
    /// claim, and a release that actually removed an entry. Distinct from
    /// `epoch`, which only moves on claims: a disconnect release changes what
    /// the spine must publish (the owner field clears) without assigning any
    /// new ownership, so the spine check needs a counter that moves for it too.
    /// Read by [`PtySizeOwners::ownership_generation`] as the cheap "did
    /// ownership change since the last spine check" signal; the fingerprint
    /// compare downstream remains the precise emit gate.
    pub(crate) generation: u64,
}

/// Tracks which connection currently owns sizing+input for each PTY, keyed by
/// pty id (the tab id for an agent PTY — the session id for the session-slot
/// tab — and the terminal id for a companion). The most recently CLAIMING
/// connection owns it; a resize from a non-owner is ignored, which breaks the
/// last-writer-wins feedback loop two viewers of one PTY would otherwise
/// create, and a non-owner's stdin is dropped. Shared between every per-PTY
/// socket (via [`crate::server::AppState`]) and the engine actor loop (via
/// [`crate::engine_actor::EngineHandle`]), which is why
/// [`crate::engine_actor::build_actor_channels`] constructs it.
#[derive(Default)]
pub(crate) struct PtySizeOwners {
    pub(crate) owners: std::sync::Mutex<OwnersState>,
    pub(crate) next_conn_id: std::sync::atomic::AtomicU64,
}

/// Outcome of [`PtySizeOwners::may_write`]: whether the connection may forward its
/// stdin to the PTY (`allowed`), whether the check itself NEWLY claimed an unowned
/// PTY (`claimed_new`) so the caller emits exactly one `pty.owner` handover for that
/// uncontested first write, and the ownership `epoch` assigned for that new claim
/// (`Some` iff `claimed_new`) so the emitted handover carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WriteClaim {
    pub(crate) allowed: bool,
    pub(crate) claimed_new: bool,
    pub(crate) epoch: Option<u64>,
}

impl PtySizeOwners {
    /// Allocate a process-unique id for a freshly attached PTY socket, used to
    /// compare against the recorded owner.
    pub(crate) fn next_conn_id(&self) -> u64 {
        self.next_conn_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Make `conn_id` the current sizing+input owner of `pty_id`. A client claims
    /// ownership by sending a size frame, so the most recently claiming connection
    /// wins, taking over from any prior owner. Attaching alone no longer claims:
    /// a backgrounded tab that attaches as a silent observer (sends no size) never
    /// steals ownership from the foregrounded device. Returns whether the owner
    /// CHANGED, returning `Some(epoch)` with the new ownership epoch on a real
    /// handover and `None` on a same-owner re-claim (so the caller broadcasts a
    /// `pty.owner` only on a real handover, stamping the returned epoch onto it).
    /// The epoch is assigned UNDER the owners lock, so it orders concurrent claims
    /// by their true serialization order.
    pub(crate) fn claim(&self, pty_id: &str, conn_id: u64) -> Option<u64> {
        let mut owners = self.owners.lock().unwrap();
        if owners.map.get(pty_id) == Some(&conn_id) {
            return None;
        }
        owners.map.insert(pty_id.to_string(), conn_id);
        owners.epoch += 1;
        owners.generation += 1;
        Some(owners.epoch)
    }

    /// Whether `conn_id` is the current owner of `pty_id`. Unlike [`claim`] this
    /// never mutates: an unowned PTY (no client has sent a size yet) returns false.
    /// A read-only ownership probe used by tests to assert the post-conditions of
    /// [`claim`], [`may_write`], and [`release`]; the live handler gates stdin
    /// through [`may_write`] (atomic) and resize through [`claim`], so production
    /// never needs a separate non-mutating check.
    ///
    /// [`claim`]: PtySizeOwners::claim
    /// [`may_write`]: PtySizeOwners::may_write
    /// [`release`]: PtySizeOwners::release
    #[cfg(test)]
    pub(crate) fn is_owner(&self, pty_id: &str, conn_id: u64) -> bool {
        self.owners.lock().unwrap().map.get(pty_id) == Some(&conn_id)
    }

    /// Decide whether `conn_id` may write stdin to `pty_id`, resolving the gate
    /// ATOMICALLY under the owners lock so no concurrent [`claim`] can slip between
    /// the decision and the write (the TOCTOU window a separate `is_owner`-then-write
    /// left open: a just-demoted connection's keystroke could still reach the PTY).
    /// Semantics:
    ///   - no current owner -> `conn_id` becomes the owner (an uncontested first
    ///     writer claims, mirroring how a size frame auto-claims an unowned PTY),
    ///     reported via `claimed_new` so the caller emits exactly one `pty.owner`
    ///     handover. This restores input for a solo/out-of-band client whose stdin
    ///     arrives before any size frame (previously silently dropped).
    ///   - owner == conn_id -> allowed, no handover.
    ///   - a different owner -> denied; the non-owner's stdin is dropped so a
    ///     read-only secondary viewer can never disrupt the active device's typing.
    ///
    /// Unlike a size frame's [`claim`] (most-recent-wins, which DOES take over an
    /// existing owner), writing never steals control from another owner: typing must
    /// not silently wrest the prompt away from the active device.
    ///
    /// [`claim`]: PtySizeOwners::claim
    pub(crate) fn may_write(&self, pty_id: &str, conn_id: u64) -> WriteClaim {
        let mut owners = self.owners.lock().unwrap();
        match owners.map.get(pty_id) {
            Some(&owner) if owner == conn_id => WriteClaim {
                allowed: true,
                claimed_new: false,
                epoch: None,
            },
            Some(_) => WriteClaim {
                allowed: false,
                claimed_new: false,
                epoch: None,
            },
            None => {
                owners.map.insert(pty_id.to_string(), conn_id);
                owners.epoch += 1;
                owners.generation += 1;
                WriteClaim {
                    allowed: true,
                    claimed_new: true,
                    epoch: Some(owners.epoch),
                }
            }
        }
    }

    /// Release ownership of `pty_id` if `conn_id` still holds it (called when the
    /// connection disconnects). A no-op if another connection has since claimed it,
    /// so a later attach is never clobbered.
    pub(crate) fn release(&self, pty_id: &str, conn_id: u64) {
        let mut owners = self.owners.lock().unwrap();
        if owners.map.get(pty_id) == Some(&conn_id) {
            owners.map.remove(pty_id);
            owners.generation += 1;
        }
    }

    /// The mutation counter for the owner map, read by the engine actor's spine
    /// check as its cheap "ownership might have changed" gate signal, exactly
    /// like `mutation_version` and `streaming_version`. See
    /// [`OwnersState::generation`] for why this is not `epoch`.
    pub(crate) fn ownership_generation(&self) -> u64 {
        self.owners.lock().unwrap().generation
    }

    /// A point-in-time copy of the owner map (pty id -> owning connection id),
    /// taken by the spine check when it actually runs a fingerprint compare so
    /// the overlay stamps a CONSISTENT set of owners onto one spine build. A
    /// clone rather than a borrow: the map is small (one entry per driven PTY)
    /// and the lock must not be held across the spine projection.
    pub(crate) fn input_owners_snapshot(&self) -> std::collections::HashMap<String, u64> {
        self.owners.lock().unwrap().map.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generation is the spine check's gate signal, so it must move on every
    /// shape of map mutation: a size-frame claim, a handover claim over an
    /// existing owner, a first-writer claim, and a release that removed the
    /// entry. Each of those changes what the spine publishes.
    #[test]
    fn ownership_generation_moves_on_every_map_mutation() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();

        let g0 = owners.ownership_generation();
        assert!(owners.claim("s1", a).is_some(), "first claim is a change");
        let g1 = owners.ownership_generation();
        assert!(
            g1 > g0,
            "a claim of an unowned pty must bump the generation"
        );

        assert!(owners.claim("s1", b).is_some(), "handover is a change");
        let g2 = owners.ownership_generation();
        assert!(g2 > g1, "a handover claim must bump the generation");

        owners.release("s1", b);
        let g3 = owners.ownership_generation();
        assert!(
            g3 > g2,
            "a release that removed the owner must bump the generation"
        );

        let claim = owners.may_write("s2", a);
        assert!(claim.claimed_new, "first write claims the unowned pty");
        assert!(
            owners.ownership_generation() > g3,
            "a first-writer claim must bump the generation"
        );
    }

    /// No-op operations must NOT bump the generation, or every keystroke of the
    /// owner would churn the spine check (the exact per-write-stamp churn the
    /// spine field was designed to avoid).
    #[test]
    fn ownership_generation_ignores_no_op_operations() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();

        assert!(owners.claim("s1", a).is_some());
        let g = owners.ownership_generation();

        assert!(owners.claim("s1", a).is_none(), "same-owner re-claim");
        assert!(owners.may_write("s1", a).allowed, "owner keystroke");
        assert!(!owners.may_write("s1", b).allowed, "denied non-owner write");
        owners.release("s1", b);
        // A release by a connection that does not hold the pty removes nothing.

        assert_eq!(
            owners.ownership_generation(),
            g,
            "re-claims, ordinary writes, denied writes and no-op releases must \
             not move the generation"
        );
    }

    /// The snapshot is what the spine overlay stamps onto the view: it must
    /// reflect the live map, and clear on release.
    #[test]
    fn input_owners_snapshot_tracks_claim_and_release() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();

        assert!(owners.input_owners_snapshot().is_empty());
        owners.claim("s1", a);
        assert_eq!(owners.input_owners_snapshot().get("s1"), Some(&a));
        owners.release("s1", a);
        assert!(
            owners.input_owners_snapshot().is_empty(),
            "a disconnected owner must vanish from the published set"
        );
    }
}
