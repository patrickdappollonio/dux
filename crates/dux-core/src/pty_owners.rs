//! The per-PTY input-ownership registry: who currently holds the right to type
//! into a PTY and to decide its grid.
//!
//! It lives in core because both surfaces arbitrate through it: while the
//! background server serves behind the terminal UI, that terminal UI is a
//! registered participant too. `dux-web` re-exports it.

/// One recorded owner: the connection id that drives the pty, plus the raw
/// `User-Agent` it presented. The device rides in the same map entry as the id
/// so [`PtySizeOwners::current_owner`] can hand the handshake a device label
/// under one lock acquisition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerRecord {
    pub conn_id: u64,
    /// The owning connection's raw `User-Agent`, already length-bounded by the
    /// capture at the upgrade; `None` when it sent none.
    pub device: Option<String>,
}

/// The owner map plus the monotonic ownership epoch, guarded by ONE mutex so an
/// epoch is assigned in the same critical section that records a new owner:
/// epochs then follow true claim order even when two connections claim at once.
#[derive(Default)]
pub struct OwnersState {
    /// pty id -> the connection that currently owns sizing+input.
    pub map: std::collections::HashMap<String, OwnerRecord>,
    /// Bumped on every ownership change, a release included, and stamped onto
    /// the emitted `pty.owner` so clients order arrivals by it. Never decreases.
    pub epoch: u64,
    /// Bumped on every mutation of `map`. Feeds the spine check's cheap "did
    /// ownership change" gate ([`PtySizeOwners::ownership_generation`]), while
    /// `epoch` travels on the wire to order client-side arrivals.
    pub generation: u64,
    /// Per-pty resize sequence, stamped under this lock on every applying resize
    /// so receivers can drop a grid broadcast the runtime published out of order
    /// after the lock released. Never decreases within a process.
    pub grid_seq: std::collections::HashMap<String, u64>,
    /// Per-pty high-water mark of the resize that actually REACHED the child,
    /// with the geometry it carried. The accept and the `TIOCSWINSZ` are not one
    /// critical section (see [`PtySizeOwners::accept_grid_apply`]), so an
    /// overtaken apply site re-applies the winner's geometry from here.
    pub applied: std::collections::HashMap<String, AppliedGrid>,
}

/// The last resize that reached a pty's child: the seq it was stamped with and
/// the geometry it carried. See [`OwnersState::applied`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppliedGrid {
    pub seq: u64,
    pub rows: u16,
    pub cols: u16,
}

/// Result of applying a stamped PTY grid through the shared cross-surface
/// ordering gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridApplyOutcome<T> {
    /// A newer grid already reached the child, so the supplied apply callback
    /// was not called.
    Dropped,
    /// This grid was accepted and applied. `superseding_grid` names the newer
    /// geometry that was re-applied when this call was overtaken while its
    /// callback was touching the child.
    Applied {
        result: T,
        superseding_grid: Option<(u16, u16)>,
    },
}

/// Tracks which connection owns sizing and input for each PTY, keyed by pty id:
/// the tab id for an agent PTY (`AgentSession::slot_tab_id` for its first tab),
/// the terminal id for a companion. Built once per serve.
///
/// Attaching never steals. A plain resize claims only an UNOWNED pty; against
/// one another connection holds it is refused whole, resize included (see
/// [`Self::claim_for_resize`]). Only a resize carrying the take-over flag
/// transfers ownership, and a non-owner's stdin is dropped by
/// [`Self::write_if_owner`] and [`Self::may_write`].
#[derive(Default)]
pub struct PtySizeOwners {
    pub owners: std::sync::Mutex<OwnersState>,
}

/// The source of every connection id in the process.
///
/// Process-global, not per-registry: the background-server toggle can build
/// several registries in one run, and the ghost self-succession rule compares
/// raw ids, so an id must never repeat while the process lives.
static NEXT_CONN_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Outcome of [`PtySizeOwners::may_write`]: whether the stdin may be forwarded,
/// whether the check itself newly claimed an unowned PTY (so the caller emits
/// one `pty.owner` handover), and the epoch for that claim (`Some` iff
/// `claimed_new`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteClaim {
    pub allowed: bool,
    pub claimed_new: bool,
    pub epoch: Option<u64>,
}

/// The outcome of [`PtySizeOwners::claim_for_resize`], decided in ONE critical
/// section.
///
/// `apply` and `epoch` are independent: an owner resizing its own PTY applies
/// with no handover, a non-owner's plain resize is refused whole, and an unowned
/// pty or an explicit take-over both applies and hands over. `seq` is `Some`
/// exactly when `apply` is (see [`OwnersState::grid_seq`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeClaim {
    pub apply: bool,
    pub epoch: Option<u64>,
    pub seq: Option<u64>,
}

impl PtySizeOwners {
    /// Allocate a process-unique id for a freshly attached PTY socket. Drawn
    /// from [`NEXT_CONN_ID`], so ids stay unique across serve cycles.
    pub fn next_conn_id(&self) -> u64 {
        NEXT_CONN_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Who owns `pty_id` right now, if anyone, plus the ownership epoch and the
    /// owner's device label, all read under one lock acquisition.
    ///
    /// The PTY handshake is the only way a client that merely attached learns it
    /// is a watcher and which device is driving, because a refused claim emits
    /// nothing. The epoch lets a client that already applied a higher-stamped
    /// `pty.owner` keep the newer verdict, since the handshake and the
    /// broadcasts ride different connections with no ordering between them.
    pub fn current_owner(&self, pty_id: &str) -> (Option<u64>, u64, Option<String>) {
        let owners = self.owners.lock().unwrap();
        let record = owners.map.get(pty_id);
        (
            record.map(|r| r.conn_id),
            owners.epoch,
            record.and_then(|r| r.device.clone()),
        )
    }

    /// May `conn_id` resize `pty_id`, and does doing so hand it ownership?
    ///
    ///   - unowned              -> claims it, resize applies
    ///   - owned by `conn_id`   -> resize applies, no handover
    ///   - owned by another AND `takeover` -> ownership transfers, resize applies
    ///   - owned by another, plain resize  -> REFUSED: nothing applied, nothing
    ///     broadcast
    ///   - `takeover` AND `expected_owner` names anyone but the current owner
    ///     (an unowned pty included) -> REFUSED exactly as a plain resize is
    ///
    /// `expected_owner` is a compare-and-swap on a flagged claim, ignored
    /// entirely when `takeover` is false. `None` takes from whoever holds it,
    /// which is what a pressed Take over sends; `Some(id)` narrows the claim to
    /// one predecessor, for the press-less re-claim of a returning owner
    /// succeeding its own ghost. The comparison runs in the same critical
    /// section as the claim, so a frame delayed on a mobile radio cannot steal a
    /// pty somebody legitimately claimed in the gap.
    ///
    /// `apply_resize` runs under the owners lock, exactly when the answer is
    /// "apply", so the recorded owner and the geometry the child was last told
    /// cannot serialize in opposite orders. It is handed the seq stamped for this
    /// resize, to offer to [`Self::accept_grid_apply`] at the apply site.
    ///
    /// `device` is the claiming connection's captured `User-Agent`, recorded on
    /// a claim and ignored on every other outcome.
    pub fn claim_for_resize(
        &self,
        pty_id: &str,
        conn_id: u64,
        takeover: bool,
        expected_owner: Option<u64>,
        device: Option<&str>,
        apply_resize: impl FnOnce(u64),
    ) -> ResizeClaim {
        let refused = ResizeClaim {
            apply: false,
            epoch: None,
            seq: None,
        };
        let mut owners = self.owners.lock().unwrap();
        let mut outcome = match owners.map.get(pty_id) {
            Some(record) if record.conn_id == conn_id => ResizeClaim {
                apply: true,
                epoch: None,
                seq: None,
            },
            Some(_) if !takeover => refused,
            // A flagged claim naming a predecessor must find exactly that
            // predecessor recorded; anything else, an unowned pty included,
            // means the client's premise was already overtaken.
            current
                if takeover
                    && expected_owner
                        .is_some_and(|expected| current.map(|r| r.conn_id) != Some(expected)) =>
            {
                refused
            }
            _ => {
                owners.map.insert(
                    pty_id.to_string(),
                    OwnerRecord {
                        conn_id,
                        device: device.map(str::to_owned),
                    },
                );
                owners.epoch += 1;
                owners.generation += 1;
                ResizeClaim {
                    apply: true,
                    epoch: Some(owners.epoch),
                    seq: None,
                }
            }
        };
        if outcome.apply {
            // Stamp the seq in the SAME critical section that enqueues the
            // resize, so seq order is apply order.
            let seq = owners.grid_seq.entry(pty_id.to_string()).or_insert(0);
            *seq += 1;
            let seq = *seq;
            outcome.seq = Some(seq);
            apply_resize(seq);
        }
        outcome
    }

    /// Hand `pty_id` to `conn_id` unconditionally and report the handover epoch
    /// (`None` when it already owned it). A thin spelling of
    /// [`Self::claim_for_resize`] with `takeover: true` and nothing to resize, so
    /// there is one implementation of "record a new owner". Deliberately not
    /// `#[cfg(test)]`: its callers live in more than one crate.
    pub fn claim(&self, pty_id: &str, conn_id: u64) -> Option<u64> {
        self.claim_for_resize(pty_id, conn_id, true, None, None, |_| {})
            .epoch
    }

    /// Whether `conn_id` is the current owner of `pty_id`; an unowned PTY is
    /// false. Read-only, unlike [`Self::claim`]. Never use it to gate a write or
    /// a resize: those go through [`Self::write_if_owner`], [`Self::may_write`]
    /// and [`Self::claim_for_resize`], which decide and act under one lock, and a
    /// check here followed by an action leaves a TOCTOU window open.
    pub fn is_owner(&self, pty_id: &str, conn_id: u64) -> bool {
        self.owners
            .lock()
            .unwrap()
            .map
            .get(pty_id)
            .is_some_and(|record| record.conn_id == conn_id)
    }

    /// May `conn_id` write stdin to `pty_id`, and does writing claim it?
    ///
    /// `enqueue` runs under the owners lock, exactly when the answer is
    /// "allowed", so a take-over cannot land between the verdict and the bytes
    /// the way it can when a caller writes after this returns. It carries the
    /// same contract [`Self::write_if_owner`] states: a cheap enqueue that never
    /// blocks and never panics. A caller with nothing to enqueue here (a launch
    /// claiming its own child, a test) passes an empty closure and this is the
    /// decision alone.
    ///
    ///   - no current owner -> `conn_id` claims it, as an uncontested first
    ///     writer, reported via `claimed_new` so the caller emits exactly one
    ///     `pty.owner` handover
    ///   - owner == conn_id -> allowed, no handover
    ///   - a different owner -> denied, the stdin is dropped
    ///
    /// Writing never steals from another owner, and neither does a plain resize:
    /// the one frame that transfers ownership is a resize flagged as a take-over
    /// (see [`Self::claim_for_resize`]). `device` is the writing connection's
    /// captured `User-Agent`, recorded only on the claim.
    pub fn may_write(
        &self,
        pty_id: &str,
        conn_id: u64,
        device: Option<&str>,
        enqueue: impl FnOnce(),
    ) -> WriteClaim {
        let mut owners = self.owners.lock().unwrap();
        let claim = match owners.map.get(pty_id) {
            Some(record) if record.conn_id == conn_id => WriteClaim {
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
                owners.map.insert(
                    pty_id.to_string(),
                    OwnerRecord {
                        conn_id,
                        device: device.map(str::to_owned),
                    },
                );
                owners.epoch += 1;
                owners.generation += 1;
                WriteClaim {
                    allowed: true,
                    claimed_new: true,
                    epoch: Some(owners.epoch),
                }
            }
        };
        if claim.allowed {
            enqueue();
        }
        claim
    }

    /// Write into `pty_id` as `conn_id`, deciding and enqueuing in ONE critical
    /// section, and report whether the bytes were handed over.
    ///
    /// It is allowed only for the connection that already owns the pty: it never
    /// claims and never transfers, so an unowned pty is refused here and claimed
    /// only through [`Self::may_write`] or [`Self::claim_for_resize`].
    ///
    /// `write` runs with the owners lock held, so it must be the cheap enqueue a
    /// keystroke already is (a channel send), never blocking I/O, and it must not
    /// panic: this mutex guards every pty in the process, and a panic through it
    /// poisons ownership for all of them. That is what closes the window a
    /// separate check leaves open, in which a take-over lands between the verdict
    /// and the write and one keystroke reaches a pty the writer no longer owns.
    pub fn write_if_owner(&self, pty_id: &str, conn_id: u64, write: impl FnOnce()) -> bool {
        let owners = self.owners.lock().unwrap();
        let allowed = owners
            .map
            .get(pty_id)
            .is_some_and(|record| record.conn_id == conn_id);
        if allowed {
            write();
        }
        allowed
    }

    /// Release ownership of `pty_id` if `conn_id` still holds it (called when the
    /// connection disconnects). A no-op if another connection has since claimed it,
    /// so a later attach is never clobbered.
    ///
    /// `Some(epoch)` means an owner really was cleared and the caller must
    /// broadcast an owner-cleared `pty.owner`: it is a viewer's only way to
    /// learn the driving device has gone, so without it the take-over card is a
    /// permanent lie. The release takes an epoch because a client discards a
    /// `pty.owner` that is not newer than what it has applied.
    pub fn release(&self, pty_id: &str, conn_id: u64) -> Option<u64> {
        let mut owners = self.owners.lock().unwrap();
        if owners
            .map
            .get(pty_id)
            .is_none_or(|record| record.conn_id != conn_id)
        {
            return None;
        }
        owners.map.remove(pty_id);
        owners.epoch += 1;
        owners.generation += 1;
        Some(owners.epoch)
    }

    /// Release every pty `conn_id` still owns, each paired with the epoch its
    /// release was stamped with, in the order the announcements must be
    /// published in.
    ///
    /// The terminal UI can be driving several ptys at once and lets go of all of
    /// them at one moment. One critical section rather than a loop of
    /// [`Self::release`] calls keeps the epochs consecutive and stops a browser
    /// claiming one mid-sweep only to have that claim released underneath it.
    pub fn release_all(&self, conn_id: u64) -> Vec<(String, u64)> {
        let mut owners = self.owners.lock().unwrap();
        let held: Vec<String> = owners
            .map
            .iter()
            .filter(|(_, record)| record.conn_id == conn_id)
            .map(|(pty_id, _)| pty_id.clone())
            .collect();
        let mut released = Vec::with_capacity(held.len());
        for pty_id in held {
            owners.map.remove(&pty_id);
            owners.epoch += 1;
            owners.generation += 1;
            released.push((pty_id, owners.epoch));
        }
        released
    }

    /// May a resize stamped with `seq` still reach the child of `pty_id`?
    ///
    /// Every surface stamps under this lock in claim order, but they do not apply
    /// at the same moment: a browser's resize is enqueued to the engine actor
    /// while the terminal UI applies at once, so a resize stamped first can reach
    /// the child last and nothing afterwards would correct the geometry. A seq
    /// not strictly newer than the last one that landed is therefore dropped,
    /// which loses nothing: it is superseded by definition, and the handshake and
    /// grid broadcast both report what the child was actually told.
    ///
    /// The accept and the child's resize are deliberately two critical sections:
    /// `PtyClient::resize` takes the terminal lock, which a replay build can hold
    /// for tens of milliseconds, so holding this lock behind it would stall every
    /// keystroke gate. [`Self::superseding_grid`] closes the window that leaves,
    /// which is why `rows` and `cols` are recorded here.
    pub fn accept_grid_apply(&self, pty_id: &str, seq: u64, rows: u16, cols: u16) -> bool {
        let mut owners = self.owners.lock().unwrap();
        let landed = owners.applied.entry(pty_id.to_string()).or_default();
        if seq <= landed.seq {
            return false;
        }
        *landed = AppliedGrid { seq, rows, cols };
        true
    }

    /// The geometry of an apply that overtook `seq`, or `None` when `seq` is
    /// still the newest one accepted for `pty_id`.
    ///
    /// Asked immediately AFTER resizing the child: `Some` means another surface's
    /// grid was accepted mid-`TIOCSWINSZ` and the child is sized for the loser,
    /// so re-applying the returned geometry is the fix. It terminates, because
    /// the winner's own check returns `None`.
    pub fn superseding_grid(&self, pty_id: &str, seq: u64) -> Option<(u16, u16)> {
        let owners = self.owners.lock().unwrap();
        let landed = owners.applied.get(pty_id)?;
        (landed.seq > seq).then_some((landed.rows, landed.cols))
    }

    /// Apply a stamped grid in the one order shared by every surface.
    ///
    /// The callback runs without the ownership lock held. If a newer grid is
    /// accepted meanwhile, `before_heal` runs and the callback is invoked once
    /// more with the winner's geometry so the child converges on it.
    pub fn apply_grid_in_order<T>(
        &self,
        pty_id: &str,
        seq: u64,
        rows: u16,
        cols: u16,
        mut apply: impl FnMut(u16, u16) -> T,
        mut before_heal: impl FnMut(u16, u16),
    ) -> GridApplyOutcome<T> {
        if !self.accept_grid_apply(pty_id, seq, rows, cols) {
            return GridApplyOutcome::Dropped;
        }

        let result = apply(rows, cols);
        let superseding_grid = self.superseding_grid(pty_id, seq);
        if let Some((winner_rows, winner_cols)) = superseding_grid {
            before_heal(winner_rows, winner_cols);
            let _ = apply(winner_rows, winner_cols);
        }

        GridApplyOutcome::Applied {
            result,
            superseding_grid,
        }
    }

    /// The per-pty STAMPED grid sequence: the seq of the last granted claim, or
    /// 0 before any. Deliberately not the last applied seq
    /// ([`Self::applied_grid_seq`]): the two differ while a stamped resize is
    /// still queued, and seeding a handshake's drop filter from this one drops
    /// the not-yet-published broadcast forever.
    pub fn grid_seq(&self, pty_id: &str) -> u64 {
        self.owners
            .lock()
            .unwrap()
            .grid_seq
            .get(pty_id)
            .copied()
            .unwrap_or(0)
    }

    /// The seq of the last resize that actually REACHED the child, or 0 before
    /// any.
    ///
    /// The valid seed for a PTY-socket handshake's grid-drop filter, and a valid
    /// lower bound for the grid that handshake carries. The stamped seq is not:
    /// a resize that has not reached the child yet would make the client drop
    /// the very broadcast announcing its apply, permanently.
    pub fn applied_grid_seq(&self, pty_id: &str) -> u64 {
        self.owners
            .lock()
            .unwrap()
            .applied
            .get(pty_id)
            .map(|landed| landed.seq)
            .unwrap_or(0)
    }

    /// The owner-map mutation counter, read by the engine actor's spine check as
    /// its cheap "ownership might have changed" gate. See
    /// [`OwnersState::generation`] for why this is not `epoch`.
    pub fn ownership_generation(&self) -> u64 {
        self.owners.lock().unwrap().generation
    }

    /// A point-in-time copy of the owner map (pty id -> owning connection id),
    /// so the overlay stamps a consistent set of owners onto one spine build.
    /// Cloned because the lock must not be held across the spine projection.
    pub fn input_owners_snapshot(&self) -> std::collections::HashMap<String, u64> {
        self.owners
            .lock()
            .unwrap()
            .map
            .iter()
            .map(|(pty_id, record)| (pty_id.clone(), record.conn_id))
            .collect()
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
        claim_resize_expecting(owners, pty, conn, takeover, None)
    }

    /// The same, with an `expected_owner` compare-and-swap in play.
    fn claim_resize_expecting(
        owners: &PtySizeOwners,
        pty: &str,
        conn: u64,
        takeover: bool,
        expected_owner: Option<u64>,
    ) -> (ResizeClaim, bool) {
        let applied = std::cell::Cell::new(false);
        let outcome = owners.claim_for_resize(pty, conn, takeover, expected_owner, None, |_| {
            applied.set(true)
        });
        (outcome, applied.get())
    }

    /// Two serve cycles must not reuse connection ids.
    ///
    /// A registry is built per serve (`build_actor_channels` constructs it), and
    /// the background-server toggle can build several in one process. When the
    /// counter lived on the registry, cycle two handed out 0, 1, 2 again, so the
    /// ghost self-succession rule ("this pane's previous, dead connection id was
    /// mine") could recognise a DIFFERENT device's id as its own ghost and
    /// transfer ownership to the wrong browser. The ids are therefore drawn from
    /// one process-global counter.
    #[test]
    fn conn_ids_are_disjoint_across_two_serve_cycles() {
        let first = PtySizeOwners::default();
        let cycle_one = [first.next_conn_id(), first.next_conn_id()];
        drop(first);
        let second = PtySizeOwners::default();
        let cycle_two = [second.next_conn_id(), second.next_conn_id()];
        for id in cycle_one {
            assert!(
                !cycle_two.contains(&id),
                "a second serve cycle reissued connection id {id} from the first: \
                 {cycle_one:?} vs {cycle_two:?}"
            );
        }
    }

    /// THE HANDSHAKE'S OWNER AND THE RECORDED OWNER ARE ONE NUMBER, which is the
    /// whole basis of self-succession: a returning pane recognises its own dead
    /// connection in the `connected` frame and names it back as the predecessor
    /// it expects to displace. If the id a socket is told and the id written down
    /// at claim time could differ, every succession would name a ghost the server
    /// has never heard of and be refused.
    #[test]
    fn the_owner_snapshot_reports_exactly_the_id_that_claimed_it() {
        let owners = PtySizeOwners::default();
        let pane = owners.next_conn_id();
        let (out, _) = claim_resize(&owners, "p", pane, false);
        assert!(out.apply);
        let (owner, _epoch, _device) = owners.current_owner("p");
        assert_eq!(
            owner,
            Some(pane),
            "the handshake reports this field, so it must be the claimer's own id"
        );

        // And that is the value `expected_owner` is matched against: nothing else.
        let successor = owners.next_conn_id();
        let stranger = owners.next_conn_id();
        let refused = owners.claim_for_resize("p", successor, true, Some(stranger), None, |_| {});
        assert!(!refused.apply, "a succession naming anyone else is refused");
        let granted = owners.claim_for_resize("p", successor, true, Some(pane), None, |_| {});
        assert!(
            granted.apply,
            "a succession naming the recorded owner is granted"
        );
    }

    /// EVERY CONNECTION BURNS AN ID, so the numbers one pane sees are not
    /// consecutive and the gaps are not a bug. A second pane, another tab, the
    /// terminal UI's own seat while it serves in the background, and any socket
    /// that opened and closed without ever claiming all draw from the same
    /// process-global counter. A pane holding 46 while the pty is owned by 45 is
    /// therefore an ordinary sight, and it means the owner is somebody else's
    /// connection unless 45 is an id this pane itself once held.
    #[test]
    fn every_connection_burns_an_id_so_one_pane_sees_gaps() {
        let owners = PtySizeOwners::default();
        let pane_first = owners.next_conn_id();
        let _somebody_else = owners.next_conn_id();
        let pane_second = owners.next_conn_id();
        assert_eq!(pane_second, pane_first + 2);
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
        assert_eq!(out.seq, Some(1), "the first applied resize starts the seq");
        assert!(applied, "the resize applies when the claim is granted");
        assert!(owners.is_owner("p", a));

        // OWNED-BY-SELF x plain: the steady-state resize. Applies, hands over
        // nothing, so no `pty.owner` is broadcast for every divider drag.
        let (out, applied) = claim_resize(&owners, "p", a, false);
        assert_eq!(
            out,
            ResizeClaim {
                apply: true,
                epoch: None,
                seq: Some(2)
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
                epoch: None,
                seq: Some(3)
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
                epoch: None,
                seq: None
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
        let first = owners.claim_for_resize("p", a, false, None, None, |_| {
            applied.borrow_mut().push((a, 80));
        });
        let second = owners.claim_for_resize("p", b, true, None, None, |_| {
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

    /// THE SEQ the grid broadcast is ordered by: stamped under the owners lock
    /// in apply order, strictly increasing per pty across interleaved claims by
    /// different connections, absent on a refusal, and independent per pty. The
    /// broadcasts themselves are published after the lock releases and can
    /// invert on the runtime; this order is what lets a receiver drop the stale
    /// one, so it must be airtight at the source.
    #[test]
    fn grid_seq_is_monotonic_per_pty_in_apply_order_and_absent_on_refusal() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();

        assert_eq!(owners.grid_seq("p"), 0, "no applied resize yet");

        // Interleaved: A claims, B takes over, A takes back, each apply gets
        // the next seq in the order the lock granted them.
        let s1 = claim_resize(&owners, "p", a, false).0.seq;
        let s2 = claim_resize(&owners, "p", b, true).0.seq;
        let s3 = claim_resize(&owners, "p", a, true).0.seq;
        assert_eq!((s1, s2, s3), (Some(1), Some(2), Some(3)));

        // A refusal advances nothing: the resize did not land, so announcing a
        // seq for it would let a stale geometry outrank a real one.
        let (refused, _) = claim_resize(&owners, "p", b, false);
        assert_eq!(refused.seq, None);
        assert_eq!(
            owners.grid_seq("p"),
            3,
            "the accessor reports the last STAMPED seq"
        );

        // Another pty counts on its own: the broadcasts are filtered per pty,
        // so the order only has to hold within one.
        let (other, _) = claim_resize(&owners, "q", b, false);
        assert_eq!(other.seq, Some(1));
        assert_eq!(owners.grid_seq("p"), 3);
    }

    /// THE ONE APPLY ORDER, across surfaces that apply at different moments.
    ///
    /// A browser's resize is stamped under the owners lock and then ENQUEUED to
    /// the engine actor, so it lands later. The terminal UI holds the engine and
    /// applies straight away. So the earlier claim can reach the child after the
    /// later one, which is exactly the inversion `grid_seq` was invented to stop
    /// on the wire, happening this time to the child itself: the pty ends up
    /// sized for the loser while the winner is recorded as its owner and believes
    /// it has already told the child.
    ///
    /// The gate is the fix: every apply site, on either surface, offers its
    /// stamped seq here first, and a seq that is not newer than the last applied
    /// one is dropped.
    #[test]
    fn a_deferred_resize_is_dropped_when_a_later_claim_already_applied() {
        let owners = PtySizeOwners::default();
        let browser = owners.next_conn_id();
        let tui = owners.next_conn_id();
        let applied: std::cell::RefCell<Vec<(u64, u16)>> = std::cell::RefCell::new(Vec::new());

        // The browser claims the unowned pty at 80 columns and ENQUEUES its
        // resize: nothing has reached the child yet.
        let queued = owners.claim_for_resize("p", browser, false, None, None, |_| {});
        let queued_seq = queued.seq.expect("the claim applied, so it stamped a seq");

        // The terminal UI takes over at 100 columns and applies immediately.
        let direct = owners.claim_for_resize("p", tui, true, None, None, |_| {});
        let direct_seq = direct.seq.expect("the take-over applied");
        assert!(
            owners.accept_grid_apply("p", direct_seq, 30, 100),
            "the newest stamped resize is the one that may reach the child"
        );
        applied.borrow_mut().push((tui, 100));

        // Now the browser's queued resize is drained. It must not land: the
        // terminal UI owns the pty and its geometry is already on the child.
        assert!(
            !owners.accept_grid_apply("p", queued_seq, 24, 80),
            "a resize stamped before the winning claim must be dropped, not applied"
        );
        assert_eq!(
            applied.borrow().as_slice(),
            &[(tui, 100)],
            "the owner's geometry must be the last thing the child was told"
        );

        // A fresh claim by the demoted browser is newer, so it passes again: the
        // gate drops stale applies, it does not wedge the pty.
        let again = owners
            .claim_for_resize("p", browser, true, None, None, |_| {})
            .seq
            .expect("the take-over applied");
        assert!(owners.accept_grid_apply("p", again, 24, 80));
    }

    /// The gate is per pty, and it never refuses the very first apply.
    #[test]
    fn the_apply_gate_starts_open_and_counts_per_pty() {
        let owners = PtySizeOwners::default();
        let conn = owners.next_conn_id();

        let first = claim_resize(&owners, "p", conn, false).0.seq.unwrap();
        assert!(
            owners.accept_grid_apply("p", first, 24, 80),
            "nothing applied yet"
        );
        assert!(
            !owners.accept_grid_apply("p", first, 24, 80),
            "the same seq offered twice is a duplicate, not a newer geometry"
        );

        // Another pty is stamped and gated on its own counter, exactly as the
        // broadcasts are filtered per pty.
        let other = claim_resize(&owners, "q", conn, false).0.seq.unwrap();
        assert!(owners.accept_grid_apply("q", other, 24, 80));
    }

    #[test]
    fn the_shared_apply_sequence_drops_stale_work_without_calling_the_child() {
        let owners = PtySizeOwners::default();
        let browser = owners.next_conn_id();
        let tui = owners.next_conn_id();
        let stale = claim_resize(&owners, "p", browser, false).0.seq.unwrap();
        let newest = claim_resize(&owners, "p", tui, true).0.seq.unwrap();

        assert!(owners.accept_grid_apply("p", newest, 30, 100));
        let calls = std::cell::Cell::new(0);
        let outcome = owners.apply_grid_in_order(
            "p",
            stale,
            24,
            80,
            |_, _| calls.set(calls.get() + 1),
            |_, _| {},
        );

        assert_eq!(outcome, GridApplyOutcome::Dropped);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn the_shared_apply_sequence_heals_a_mid_apply_supersession() {
        let owners = PtySizeOwners::default();
        let browser = owners.next_conn_id();
        let tui = owners.next_conn_id();
        let older = claim_resize(&owners, "p", browser, false).0.seq.unwrap();
        let newer = claim_resize(&owners, "p", tui, true).0.seq.unwrap();
        let child = std::cell::Cell::new((0, 0));
        let calls = std::cell::Cell::new(0);

        let healed = std::cell::Cell::new(None);
        let outcome = owners.apply_grid_in_order(
            "p",
            older,
            24,
            80,
            |rows, cols| {
                let call = calls.get();
                calls.set(call + 1);
                if call == 0 {
                    assert!(owners.accept_grid_apply("p", newer, 50, 150));
                    child.set((50, 150));
                    // The older syscall returns after the winner and temporarily
                    // leaves the child at the losing geometry.
                    child.set((rows, cols));
                } else {
                    child.set((rows, cols));
                }
            },
            |rows, cols| healed.set(Some((rows, cols))),
        );

        assert_eq!(
            outcome,
            GridApplyOutcome::Applied {
                result: (),
                superseding_grid: Some((50, 150)),
            }
        );
        assert_eq!(calls.get(), 2);
        assert_eq!(healed.get(), Some((50, 150)));
        assert_eq!(child.get(), (50, 150));
    }

    /// THE HANDSHAKE'S SEED. A PTY socket that opens while a resize is stamped
    /// but not yet applied must seed its grid-drop filter from what REACHED the
    /// child, never from what was stamped.
    ///
    /// The terminal UI stamps its claim and applies in two steps, and a browser's
    /// resize is stamped and then queued for the engine actor, so the window is
    /// ordinary rather than exotic. Seeding at the stamped value put the filter
    /// above a broadcast that had not been published yet, so the socket and the
    /// client both dropped the apply announcement when it finally came, and
    /// nothing ever re-announced it: that viewer sat on the old grid for the life
    /// of the socket.
    #[test]
    fn the_applied_seq_is_what_a_handshake_may_seed_a_drop_filter_from() {
        let owners = PtySizeOwners::default();
        let conn = owners.next_conn_id();

        // A claim is granted and the resize is STAMPED, but nothing has applied
        // it yet: this is the window a handshake can land in.
        let stamped = claim_resize(&owners, "p", conn, false)
            .0
            .seq
            .expect("the claim was granted");
        assert_eq!(owners.grid_seq("p"), stamped, "the stamp moved");
        assert_eq!(
            owners.applied_grid_seq("p"),
            0,
            "nothing has reached the child yet, so the applied mark must not move"
        );

        // A socket opening here seeds from the APPLIED mark, so the apply's own
        // broadcast still passes its filter when it arrives.
        let seeded = owners.applied_grid_seq("p");
        assert!(owners.accept_grid_apply("p", stamped, 24, 80));
        assert!(
            stamped > seeded,
            "the apply broadcast must survive the filter this handshake seeded \
             ({stamped} vs {seeded}); seeding from the stamped seq drops it forever"
        );
    }

    /// THE WINDOW BETWEEN THE ACCEPT AND THE CHILD. The accept and the
    /// `TIOCSWINSZ` are deliberately two critical sections (holding the owners
    /// lock across the child's terminal lock was measured and rejected), so a
    /// newer apply really can overtake an older one that is already past its
    /// accept. The recorded geometry is what lets the loser notice and converge.
    #[test]
    fn an_overtaken_apply_site_can_find_the_geometry_that_overtook_it() {
        let owners = PtySizeOwners::default();
        let browser = owners.next_conn_id();
        let tui = owners.next_conn_id();
        // What the child is currently sized to, as the two apply sites see it.
        let child = std::cell::Cell::new((0u16, 0u16));

        // The browser's resize is accepted first, but the interleaving parks it
        // before it reaches the child.
        let queued = owners
            .claim_for_resize("p", browser, false, None, None, |_| {})
            .seq
            .expect("granted");
        assert!(owners.accept_grid_apply("p", queued, 24, 80));

        // The terminal UI takes over, accepts, and reaches the child first.
        let direct = owners
            .claim_for_resize("p", tui, true, None, None, |_| {})
            .seq
            .expect("granted");
        assert!(owners.accept_grid_apply("p", direct, 50, 150));
        child.set((50, 150));

        // Now the browser's `TIOCSWINSZ` finally lands, leaving the child sized
        // for a device that no longer drives it.
        child.set((24, 80));
        let superseded = owners
            .superseding_grid("p", queued)
            .expect("a newer apply was accepted while this one was in flight");
        assert_eq!(superseded, (50, 150));
        child.set(superseded);

        // The winner's own re-check finds itself newest, so the correction
        // terminates rather than ping-ponging.
        assert_eq!(owners.superseding_grid("p", direct), None);
        assert_eq!(
            child.get(),
            (50, 150),
            "the child must end up at the newest accepted geometry"
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

    /// The terminal UI drives several ptys over one seat, and lets go of all of
    /// them at once: the background server stops, or dux quits. Every release is
    /// reported with its own epoch, because each becomes its own owner-cleared
    /// broadcast, and a pty somebody else has taken over in the meantime is left
    /// exactly where it is.
    #[test]
    fn release_all_clears_only_this_participants_ptys_and_reports_each_epoch() {
        let owners = PtySizeOwners::default();
        let tui = owners.next_conn_id();
        let browser = owners.next_conn_id();

        owners.claim("agent-one", tui).expect("claimed");
        owners.claim("agent-two", tui).expect("claimed");
        owners.claim("agent-three", browser).expect("claimed");

        let released = owners.release_all(tui);
        assert_eq!(
            released.len(),
            2,
            "both of this seat's ptys let go: {released:?}"
        );
        let mut names: Vec<&str> = released.iter().map(|(id, _)| id.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["agent-one", "agent-two"]);
        let epochs: Vec<u64> = released.iter().map(|(_, epoch)| *epoch).collect();
        assert!(
            epochs.windows(2).all(|pair| pair[1] > pair[0]),
            "each release needs its own strictly newer epoch, or the client's \
             ordering discards the second one as stale: {epochs:?}"
        );
        assert!(
            owners.is_owner("agent-three", browser),
            "another participant's pty must be untouched by this sweep"
        );

        assert!(
            owners.release_all(tui).is_empty(),
            "a second sweep has nothing to release and must announce nothing"
        );
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
        assert_eq!(owners.current_owner("p"), (None, 0, None));
        let claim_epoch = owners.claim("p", a).expect("a fresh claim has an epoch");
        assert_eq!(
            owners.current_owner("p"),
            (Some(a), claim_epoch, None),
            "the handshake snapshot must carry the SAME epoch the claim's \
             pty.owner broadcast carried, or the client cannot order the two"
        );
        let cleared_epoch = owners.release("p", a).expect("the owner's release");
        assert_eq!(owners.current_owner("p"), (None, cleared_epoch, None));
    }

    /// The handshake's DEVICE half: the claimer's `User-Agent` is recorded with
    /// the owner id (whichever claim path took the pty), replaced whole by the
    /// next handover, and removed with the entry on release. It is what lets the
    /// take-over card of a client that merely attached name the driving device,
    /// because a mere attach hears no `pty.owner` broadcast at all.
    #[test]
    fn current_owner_reports_the_device_recorded_at_claim_time() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();

        // A resize claim records the claimer's device.
        let claim = owners.claim_for_resize("p", a, false, None, Some("Desktop UA"), |_| {});
        assert!(claim.epoch.is_some(), "the unowned pty was claimed");
        let (owner, _, device) = owners.current_owner("p");
        assert_eq!(owner, Some(a));
        assert_eq!(
            device.as_deref(),
            Some("Desktop UA"),
            "the handshake snapshot must name the claimer's device"
        );

        // A take-over replaces both halves together; a claimer that sent no
        // User-Agent leaves the device empty rather than inheriting the old one.
        let takeover = owners.claim_for_resize("p", b, true, None, None, |_| {});
        assert!(takeover.epoch.is_some());
        assert_eq!(
            owners.current_owner("p"),
            (Some(b), takeover.epoch.unwrap(), None)
        );

        // The release removes the device with the entry.
        let cleared = owners.release("p", b).expect("the owner's release");
        assert_eq!(owners.current_owner("p"), (None, cleared, None));

        // A first-writer claim records the device too, exactly like a resize claim.
        let write = owners.may_write("p", a, Some("Phone UA"), || {});
        assert!(write.claimed_new, "the first writer claims the unowned pty");
        let (owner, _, device) = owners.current_owner("p");
        assert_eq!(owner, Some(a));
        assert_eq!(device.as_deref(), Some("Phone UA"));
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

        let claim = owners.may_write("s2", a, None, || {});
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
        assert!(
            owners.may_write("s1", a, None, || {}).allowed,
            "owner keystroke"
        );
        assert!(
            !owners.may_write("s1", b, None, || {}).allowed,
            "denied non-owner write"
        );
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

    /// THE COMPARE-AND-SWAP arm: a flagged claim that names an EXPECTED owner
    /// only transfers when that owner still holds the pty.
    ///
    /// This is the one press-less re-claim the design keeps: a returning owner
    /// recognising the pane's previous, dead connection id as its own ghost. On
    /// a mobile network that flagged resize can be delayed for seconds, and in
    /// the meantime another device may legitimately have claimed the pty. With
    /// no expectation to check, the late frame stole it with nobody pressing
    /// anything, which is exactly what "attaching never steals" forbids.
    #[test]
    fn a_flagged_claim_naming_a_stale_expected_owner_is_refused_and_applies_nothing() {
        let owners = PtySizeOwners::default();
        let a_old = owners.next_conn_id();
        let b = owners.next_conn_id();
        let a_new = owners.next_conn_id();

        // A drives the pty, then its socket dies and B claims it.
        let (out, _) = claim_resize(&owners, "p", a_old, false);
        assert!(out.apply);
        assert!(owners.release("p", a_old).is_some());
        let applied: std::cell::RefCell<Vec<(u64, u16)>> = std::cell::RefCell::new(Vec::new());
        owners.claim_for_resize("p", b, false, None, None, |_| {
            applied.borrow_mut().push((b, 100));
        });
        assert!(owners.is_owner("p", b));

        // A comes back and its ghost-succession resize finally lands, naming the
        // dead connection it believes still owns the pty. B owns it now, so the
        // whole frame is refused: no apply, no epoch, no seq, nothing to
        // broadcast.
        let out = owners.claim_for_resize("p", a_new, true, Some(a_old), None, |_| {
            applied.borrow_mut().push((a_new, 80));
        });
        assert_eq!(
            out,
            ResizeClaim {
                apply: false,
                epoch: None,
                seq: None
            }
        );
        assert!(
            owners.is_owner("p", b),
            "a stale ghost succession must not take the pty from the device that claimed it"
        );
        assert_eq!(
            applied.borrow().as_slice(),
            &[(b, 100)],
            "the child must still be sized for the connection that owns it"
        );
    }

    /// The success arm of the same rule: the expected owner really is still
    /// recorded, so the returning owner succeeds its own ghost.
    #[test]
    fn a_flagged_claim_naming_the_live_owner_transfers() {
        let owners = PtySizeOwners::default();
        let ghost = owners.next_conn_id();
        let returning = owners.next_conn_id();

        assert!(claim_resize(&owners, "p", ghost, false).0.apply);
        let (out, applied) = claim_resize_expecting(&owners, "p", returning, true, Some(ghost));
        assert!(
            out.apply,
            "the expectation held, so the transfer is granted"
        );
        assert!(out.epoch.is_some(), "a granted transfer hands over");
        assert!(applied, "a granted transfer applies its geometry");
        assert!(owners.is_owner("p", returning));
    }

    /// A PRESSED take-over carries no expectation, so it wins whoever holds the
    /// pty. Same interleaving as the refusal above, opposite verdict: the press
    /// is the thing that makes it legitimate.
    #[test]
    fn a_pressed_takeover_carries_no_expectation_and_wins_over_the_current_owner() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();

        assert!(claim_resize(&owners, "p", b, false).0.apply);
        let (out, applied) = claim_resize_expecting(&owners, "p", a, true, None);
        assert!(out.apply && out.epoch.is_some());
        assert!(applied);
        assert!(owners.is_owner("p", a));
    }

    /// An UNOWNED pty is a MISMATCH when an expectation was named, not a free
    /// claim. The client said "I am succeeding connection N"; N is gone, so the
    /// premise is false. Refusing costs nothing: the ordinary plain attach
    /// claims the free pty a moment later with no flag at all, which is the
    /// owner's rule working as intended.
    #[test]
    fn an_unowned_pty_is_a_mismatch_for_a_named_expected_owner() {
        let owners = PtySizeOwners::default();
        let ghost = owners.next_conn_id();
        let returning = owners.next_conn_id();

        let (out, applied) = claim_resize_expecting(&owners, "p", returning, true, Some(ghost));
        assert_eq!(
            out,
            ResizeClaim {
                apply: false,
                epoch: None,
                seq: None
            }
        );
        assert!(!applied);
        assert!(owners.current_owner("p").0.is_none());

        // And the plain attach that follows claims it, unflagged.
        let (out, applied) = claim_resize(&owners, "p", returning, false);
        assert!(out.apply && out.epoch.is_some());
        assert!(applied);
    }

    /// `expected_owner` is a qualifier on the take-over flag and nothing else: a
    /// plain resize is decided exactly as it always was, so a client that sends
    /// the field on every frame cannot accidentally change its own steady-state
    /// resizes or its first attach.
    #[test]
    fn expected_owner_is_ignored_when_the_frame_is_not_a_takeover() {
        let owners = PtySizeOwners::default();
        let a = owners.next_conn_id();
        let b = owners.next_conn_id();
        let stranger = owners.next_conn_id();

        // UNOWNED x plain, with a bogus expectation: still claims.
        let (out, applied) = claim_resize_expecting(&owners, "p", a, false, Some(stranger));
        assert!(out.apply && out.epoch.is_some());
        assert!(applied);

        // OWNED-BY-SELF x plain, with a bogus expectation: still applies.
        let (out, applied) = claim_resize_expecting(&owners, "p", a, false, Some(stranger));
        assert!(out.apply && out.epoch.is_none());
        assert!(applied);

        // OWNED-BY-OTHER x plain, with an expectation that actually MATCHES:
        // still refused, because only a take-over ever transfers.
        let (out, applied) = claim_resize_expecting(&owners, "p", b, false, Some(a));
        assert_eq!(
            out,
            ResizeClaim {
                apply: false,
                epoch: None,
                seq: None
            }
        );
        assert!(!applied);
        assert!(owners.is_owner("p", a));
    }

    /// The owner's write must be decided and enqueued under one lock, or a
    /// take-over landing in the gap lets one keystroke through to a pty the
    /// writer no longer holds.
    #[test]
    fn a_takeover_cannot_land_between_the_verdict_and_the_owners_write() {
        let owners = std::sync::Arc::new(PtySizeOwners::default());
        let driver = owners.next_conn_id();
        let taker = owners.next_conn_id();
        owners.claim("p", driver);

        let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let racer = {
            let owners = owners.clone();
            let order = order.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                owners.claim_for_resize("p", taker, true, None, None, |_| {});
                order.lock().unwrap().push("takeover");
            })
        };

        let wrote = owners.write_if_owner("p", driver, || {
            // The racing take-over is running by now, and the pause hands it
            // every chance to record itself first: only the lock this closure
            // runs under can keep it behind the write.
            gate.wait();
            std::thread::sleep(std::time::Duration::from_millis(50));
            order.lock().unwrap().push("write");
        });
        racer.join().expect("racing take-over thread");

        assert!(wrote, "the recorded owner's write is delivered");
        assert_eq!(
            *order.lock().unwrap(),
            ["write", "takeover"],
            "the take-over must serialize after the write it raced"
        );
        assert!(owners.is_owner("p", taker), "the take-over still lands");
    }

    /// The first writer's claim has the same seam: the bytes that earned the
    /// claim must be enqueued under the lock that granted it, or a take-over
    /// racing the claim leaves them travelling to a pty already handed on.
    #[test]
    fn a_takeover_cannot_land_between_a_first_writer_claim_and_its_enqueue() {
        let owners = std::sync::Arc::new(PtySizeOwners::default());
        let first = owners.next_conn_id();
        let taker = owners.next_conn_id();

        let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let racer = {
            let owners = owners.clone();
            let order = order.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                owners.claim_for_resize("p", taker, true, None, None, |_| {});
                order.lock().unwrap().push("takeover");
            })
        };

        let claim = owners.may_write("p", first, None, || {
            gate.wait();
            std::thread::sleep(std::time::Duration::from_millis(50));
            order.lock().unwrap().push("enqueue");
        });
        racer.join().expect("racing take-over thread");

        assert!(claim.claimed_new, "an unowned pty's first writer claims it");
        assert_eq!(
            *order.lock().unwrap(),
            ["enqueue", "takeover"],
            "the take-over must serialize after the enqueue its claim raced"
        );
        assert!(owners.is_owner("p", taker));
    }

    /// A take-over that lands BEFORE the next keystroke drops it: the write is
    /// refused with no bytes handed over, which is the point of coupling them.
    #[test]
    fn a_write_after_a_takeover_hands_over_nothing() {
        let owners = PtySizeOwners::default();
        let driver = owners.next_conn_id();
        let taker = owners.next_conn_id();
        owners.claim("p", driver);

        let first = std::cell::Cell::new(false);
        assert!(owners.write_if_owner("p", driver, || first.set(true)));
        assert!(first.get(), "the first keystroke reaches the child");

        owners.claim("p", taker);

        let second = std::cell::Cell::new(false);
        assert!(!owners.write_if_owner("p", driver, || second.set(true)));
        assert!(
            !second.get(),
            "the second keystroke must not reach a pty the writer lost"
        );
    }

    /// Writing through this operation claims nothing, so an unowned pty stays
    /// unowned and the bytes go nowhere.
    #[test]
    fn writing_to_an_unowned_pty_is_refused_and_claims_it_for_nobody() {
        let owners = PtySizeOwners::default();
        let conn = owners.next_conn_id();

        let wrote = std::cell::Cell::new(false);
        assert!(!owners.write_if_owner("p", conn, || wrote.set(true)));
        assert!(!wrote.get());
        assert_eq!(
            owners.current_owner("p").0,
            None,
            "an unowned pty is claimed by a deliberate act, never by a write"
        );
    }
}
