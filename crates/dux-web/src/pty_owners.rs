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
    /// Bumped on every ownership CHANGE, a release included; the value handed to
    /// the caller and stamped onto the emitted `pty.owner` so clients converge on
    /// the latest claim regardless of broadcast arrival order. Never decreases
    /// within a process.
    pub(crate) epoch: u64,
    /// Bumped on every MUTATION of `map` — a claim handover, a first-writer
    /// claim, and a release that actually removed an entry. Distinct from
    /// `epoch` in what it FEEDS rather than in when it moves: this is the spine
    /// check's cheap "did ownership change" gate, read by
    /// [`PtySizeOwners::ownership_generation`], while the epoch travels on the
    /// wire to order client-side arrivals. The fingerprint compare downstream
    /// remains the precise emit gate.
    pub(crate) generation: u64,
}

/// Tracks which connection currently owns sizing+input for each PTY, keyed by
/// pty id (the tab id for an agent PTY, which is the session id for the
/// session-slot tab, and the terminal id for a companion). Shared between every per-PTY
/// socket (via [`crate::server::AppState`]) and the engine actor loop (via
/// [`crate::engine_actor::EngineHandle`]), which is why
/// [`crate::engine_actor::build_actor_channels`] constructs it.
///
/// ATTACHING NEVER STEALS. A plain resize claims only an UNOWNED pty; against a
/// pty another connection already owns it is REFUSED whole, resize included (see
/// [`Self::claim_for_resize`]). Only a resize frame that explicitly carries the
/// take-over flag transfers ownership, and only a deliberate press of Take over
/// sends one. That is the difference between this and the shape it had before:
/// a resize used to claim unconditionally, so every foregrounded attach silently
/// wrested control from whichever device was actually being typed on, and the
/// two devices then ping-ponged the pty's size at each other. A non-owner's
/// stdin is dropped by [`Self::may_write`], which never steals either.
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

/// The outcome of [`PtySizeOwners::claim_for_resize`], decided in ONE critical
/// section: whether the resize was applied to the PTY (`apply`) and, when the
/// call also transferred ownership, the epoch stamped onto the `pty.owner`
/// broadcast the caller then emits (`epoch`).
///
/// `apply` and `epoch` are deliberately independent. The current owner resizing
/// its own PTY applies without any handover (`apply: true`, `epoch: None`); a
/// non-owner's plain resize is refused whole (`apply: false`, `epoch: None`);
/// an unowned pty and an explicit take-over both apply AND hand over
/// (`apply: true`, `epoch: Some`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResizeClaim {
    pub(crate) apply: bool,
    pub(crate) epoch: Option<u64>,
}

impl PtySizeOwners {
    /// Allocate a process-unique id for a freshly attached PTY socket, used to
    /// compare against the recorded owner.
    pub(crate) fn next_conn_id(&self) -> u64 {
        self.next_conn_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Who owns `pty_id` right now, if anyone, PLUS the ownership epoch as of
    /// that same instant. Read once per PTY socket, at the handshake, so the
    /// `connected` frame can tell the arriving client whether it is joining as
    /// the driver or as a watcher. Without it a refused claim (which emits
    /// nothing at all, by design) would leave a client that guessed "I am
    /// foregrounded, so I must be the owner" wedged as a phantom owner: typing
    /// surfaces up, every keystroke dropped server-side, and no card ever
    /// shown. Post-handshake changes reach the client through `pty.owner`.
    ///
    /// The epoch travels with the owner under ONE lock acquisition, because the
    /// handshake and the `pty.owner` broadcasts ride two different TCP
    /// connections with no ordering between them. A client that has already
    /// applied a `pty.owner` stamped with a HIGHER epoch knows this snapshot is
    /// stale and keeps the newer verdict; without the epoch, a fresh
    /// `pty.owner{owner:B}` followed by a slow `connected{owner:null}` would
    /// re-seed the client as a phantom owner, and nothing would ever correct it
    /// (the stale-null direction emits no further event).
    pub(crate) fn current_owner(&self, pty_id: &str) -> (Option<u64>, u64) {
        let owners = self.owners.lock().unwrap();
        (owners.map.get(pty_id).copied(), owners.epoch)
    }

    /// THE ONE ATOMIC DECISION behind every resize frame: may `conn_id` resize
    /// `pty_id`, and does doing so hand it ownership?
    ///
    ///   - unowned              -> claims it, resize applies
    ///   - owned by `conn_id`   -> resize applies, no handover
    ///   - owned by another AND `takeover` -> ownership transfers, resize applies
    ///   - owned by another, plain resize  -> REFUSED: nothing is applied and
    ///     nothing is broadcast (the caller logs it at debug, like a dropped
    ///     non-owner keystroke)
    ///
    /// `apply_resize` is invoked, under the owners lock, exactly when the answer
    /// is "apply". Passing the effect in rather than letting the caller act on
    /// the returned verdict is not decoration: the recorded owner and the
    /// geometry the child was last told must agree. If two connections raced
    /// with the decision and the apply split apart, claim A / claim B could
    /// serialize as A-then-B in the owner map while the resizes landed as
    /// B-then-A, leaving the pty sized for A with B recorded as its driver, and
    /// nothing would ever correct it (B believes it already told the child its
    /// size). That race is real, not theoretical: every PTY socket is its own
    /// tokio task, claims serialize on this mutex, and
    /// [`crate::engine_actor::EngineHandle::resize_pty`] is a separate `try_send`
    /// into the engine actor's queue with nothing binding the two orders.
    /// Enqueuing the resize INSIDE this critical section binds them: `try_send`
    /// never blocks and the actor drains its queue in order, so the resizes come
    /// out in claim order and the epoch winner's geometry is the one that lands
    /// last, always. The lock is therefore held for a fixed, tiny window and
    /// never across an await. This mirrors the precedent set by
    /// [`Self::may_write`], which likewise resolves the stdin gate under the lock
    /// rather than checking and then writing.
    pub(crate) fn claim_for_resize(
        &self,
        pty_id: &str,
        conn_id: u64,
        takeover: bool,
        apply_resize: impl FnOnce(),
    ) -> ResizeClaim {
        let mut owners = self.owners.lock().unwrap();
        let outcome = match owners.map.get(pty_id) {
            Some(&owner) if owner == conn_id => ResizeClaim {
                apply: true,
                epoch: None,
            },
            Some(_) if !takeover => ResizeClaim {
                apply: false,
                epoch: None,
            },
            _ => {
                owners.map.insert(pty_id.to_string(), conn_id);
                owners.epoch += 1;
                owners.generation += 1;
                ResizeClaim {
                    apply: true,
                    epoch: Some(owners.epoch),
                }
            }
        };
        if outcome.apply {
            apply_resize();
        }
        outcome
    }

    /// Hand `pty_id` to `conn_id` unconditionally, the way an explicit take-over
    /// does, and report the handover epoch (`None` when it already owned it).
    /// A thin spelling of [`Self::claim_for_resize`] with `takeover: true` and no
    /// resize to apply, so there is exactly one implementation of "record a new
    /// owner". Used by the test fixture that gives the file-drop courtesy check
    /// something to say, and by the claim-table tests.
    #[cfg(test)]
    pub(crate) fn claim(&self, pty_id: &str, conn_id: u64) -> Option<u64> {
        self.claim_for_resize(pty_id, conn_id, true, || {}).epoch
    }

    /// Whether `conn_id` is the current owner of `pty_id`. Unlike [`claim`] this
    /// never mutates: an unowned PTY (no client has sent a size yet) returns false.
    /// A read-only ownership probe used by tests to assert the post-conditions of
    /// [`claim_for_resize`], [`may_write`], and [`release`]; the live handler
    /// gates stdin through [`may_write`] (atomic) and resize through
    /// [`claim_for_resize`] (atomic), so production never needs a separate
    /// non-mutating check. The handshake's read is
    /// [`current_owner`], which answers a different question ("who", not "is it
    /// me").
    ///
    /// [`claim_for_resize`]: PtySizeOwners::claim_for_resize
    /// [`may_write`]: PtySizeOwners::may_write
    /// [`release`]: PtySizeOwners::release
    /// [`current_owner`]: PtySizeOwners::current_owner
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
    /// Writing never steals control from another owner: typing must not silently
    /// wrest the prompt away from the active device. Neither does a plain resize
    /// (see [`claim_for_resize`]); the ONE frame that transfers ownership is a
    /// resize explicitly flagged as a take-over.
    ///
    /// [`claim_for_resize`]: PtySizeOwners::claim_for_resize
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
    ///
    /// Returns `Some(epoch)` when an owner really was cleared, so the caller
    /// broadcasts an owner-cleared `pty.owner`. That broadcast is not optional
    /// bookkeeping: now that ownership no longer follows focus, a viewer told
    /// "another device is driving this" has no other way to learn that the other
    /// device has gone, and the card would be a permanent lie. The release takes
    /// an EPOCH as well as a generation bump, unlike the shape it had before,
    /// because the client orders `pty.owner` arrivals by epoch: an owner-cleared
    /// event stamped with a stale epoch would be discarded as an out-of-order
    /// duplicate and the lie would survive anyway.
    pub(crate) fn release(&self, pty_id: &str, conn_id: u64) -> Option<u64> {
        let mut owners = self.owners.lock().unwrap();
        if owners.map.get(pty_id) != Some(&conn_id) {
            return None;
        }
        owners.map.remove(pty_id);
        owners.epoch += 1;
        owners.generation += 1;
        Some(owners.epoch)
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

    /// Run `claim_for_resize` and report both the verdict and whether the resize
    /// effect actually ran, so the table below can assert that "apply" is not
    /// merely reported but obeyed.
    fn claim_resize(
        owners: &PtySizeOwners,
        pty: &str,
        conn: u64,
        takeover: bool,
    ) -> (ResizeClaim, bool) {
        let applied = std::cell::Cell::new(false);
        let outcome = owners.claim_for_resize(pty, conn, takeover, || applied.set(true));
        (outcome, applied.get())
    }

    /// THE CLAIM TABLE: {unowned, owned-by-other, owned-by-self} x {plain,
    /// takeover}. This is the whole of "attaching never steals": the only cell
    /// that takes a pty away from a live owner is the one where the client said
    /// so.
    #[test]
    fn claim_for_resize_table() {
        // UNOWNED x plain: claims, and the resize applies. This is the ordinary
        // first attach, and the only case an older client's unflagged claim was
        // ever legitimately granted.
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();
        let (out, applied) = claim_resize(&owners, "p", a, false);
        assert!(out.apply);
        assert!(out.epoch.is_some(), "an unowned pty is claimed by a resize");
        assert!(applied, "the resize applies when the claim is granted");
        assert!(owners.is_owner("p", a));

        // OWNED-BY-SELF x plain: the steady-state resize. Applies, hands over
        // nothing, so no `pty.owner` is broadcast for every divider drag.
        let (out, applied) = claim_resize(&owners, "p", a, false);
        assert_eq!(
            out,
            ResizeClaim {
                apply: true,
                epoch: None
            }
        );
        assert!(applied);

        // OWNED-BY-SELF x takeover: idempotent. The owner re-asserting itself is
        // still not a handover, so it raises no card on anybody else's screen.
        let (out, _) = claim_resize(&owners, "p", a, true);
        assert_eq!(
            out,
            ResizeClaim {
                apply: true,
                epoch: None
            }
        );

        // OWNED-BY-OTHER x plain: REFUSED WHOLE. Not just "ownership is not
        // transferred": the resize itself must not land, or a backgrounded
        // viewer's alt-tab would still SIGWINCH the owner's child to the
        // viewer's geometry, which is the visible half of the steal.
        let (out, applied) = claim_resize(&owners, "p", b, false);
        assert_eq!(
            out,
            ResizeClaim {
                apply: false,
                epoch: None
            }
        );
        assert!(!applied, "a refused resize must not reach the PTY");
        assert!(
            owners.is_owner("p", a),
            "the owner is untouched by a refusal"
        );

        // OWNED-BY-OTHER x takeover: the one transferring cell.
        let (out, applied) = claim_resize(&owners, "p", b, true);
        assert!(out.apply);
        assert!(out.epoch.is_some(), "an explicit take-over hands over");
        assert!(applied);
        assert!(owners.is_owner("p", b));

        // UNOWNED x takeover: granted too. A take-over whose target released in
        // the gap (the owner's tab closed while the card was on screen) must not
        // be refused for having nobody to take from.
        assert!(owners.release("p", b).is_some());
        let (out, applied) = claim_resize(&owners, "p", a, true);
        assert!(out.apply && out.epoch.is_some());
        assert!(applied);
    }

    /// THE RACE the atomic claim exists for: the recorded owner and the geometry
    /// the child was last told must be the SAME connection's. Two claims land
    /// back to back; whichever wins the owner map must also be the one whose
    /// resize applied last, so the child is never left painting for the loser's
    /// viewport with the winner recorded as its driver.
    ///
    /// Serialized deterministically rather than by racing threads: the property
    /// under test is that the decision and the effect share one critical
    /// section, which is exactly what "the applies come out in claim order"
    /// asserts, and a thread race would prove it only probabilistically.
    #[test]
    fn claim_for_resize_applies_in_claim_order_so_the_owner_owns_the_geometry() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();
        let applied: std::cell::RefCell<Vec<(u64, u64)>> = std::cell::RefCell::new(Vec::new());

        // A claims at 24x80, B takes over at 30x100.
        let first = owners.claim_for_resize("p", a, false, || {
            applied.borrow_mut().push((a, 80));
        });
        let second = owners.claim_for_resize("p", b, true, || {
            applied.borrow_mut().push((b, 100));
        });

        let epoch_a = first.epoch.expect("A claimed the unowned pty");
        let epoch_b = second.epoch.expect("B took it over");
        assert!(epoch_b > epoch_a, "epochs order the two claims");
        let order = applied.borrow().clone();
        assert_eq!(
            order,
            vec![(a, 80), (b, 100)],
            "the resizes must land in the same order the owner map recorded them"
        );
        let (winner, _) = *order.last().unwrap();
        assert!(
            owners.is_owner("p", winner),
            "the LAST geometry applied must belong to the connection recorded as owner"
        );
    }

    /// A release that really cleared an owner reports an epoch, so the caller can
    /// broadcast the owner-cleared `pty.owner` that stops a viewer's card from
    /// naming a device that has gone. A release by a non-owner reports nothing.
    #[test]
    fn release_reports_an_epoch_only_when_it_cleared_a_real_owner() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();

        assert_eq!(owners.release("p", a), None, "nothing to release yet");
        let claimed = owners.claim("p", a).expect("claimed");
        assert_eq!(
            owners.release("p", b),
            None,
            "a non-owner's disconnect clears nothing and announces nothing"
        );
        let cleared = owners
            .release("p", a)
            .expect("the owner's release clears it");
        assert!(
            cleared > claimed,
            "the cleared event's epoch must be strictly newer than the claim it \
             retires, or the client's epoch dedup discards it as stale"
        );
        assert!(owners.current_owner("p").0.is_none());
    }

    /// The handshake read. It is the client's only way to learn it is joining a
    /// pty somebody else is driving, because a refused claim emits nothing. The
    /// epoch rides the same snapshot so the client can tell a stale handshake
    /// from a fresh one: it must equal the epoch the claim's own `pty.owner`
    /// broadcast carried, and move again when the release retires the owner.
    #[test]
    fn current_owner_reports_the_live_owner_and_epoch_and_clears_with_it() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        assert_eq!(owners.current_owner("p"), (None, 0));
        let claim_epoch = owners.claim("p", a).expect("a fresh claim has an epoch");
        assert_eq!(
            owners.current_owner("p"),
            (Some(a), claim_epoch),
            "the handshake snapshot must carry the SAME epoch the claim's \
             pty.owner broadcast carried, or the client cannot order the two"
        );
        let cleared_epoch = owners.release("p", a).expect("the owner's release");
        assert_eq!(owners.current_owner("p"), (None, cleared_epoch));
    }

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

        let _ = owners.release("s1", b);
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
        let _ = owners.release("s1", b);
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
        let _ = owners.release("s1", a);
        assert!(
            owners.input_owners_snapshot().is_empty(),
            "a disconnected owner must vanish from the published set"
        );
    }
}
