//! The per-PTY input-ownership registry: who currently holds the right to type
//! into a PTY and to decide its grid.
//!
//! ## Why this is in `dux-core` and not in the web layer
//!
//! Ownership started as a WEB-layer concept, arbitrated between the per-PTY
//! websockets, and lived in `dux-web`. It is not one any more: while the
//! background web server runs behind a live terminal UI, the terminal UI is a
//! registered participant too, so both surfaces have to ask the same table the
//! same questions and get answers that agree. A rule that both surfaces obey
//! belongs in the crate both surfaces can see, and this type is pure `std`
//! (a mutex, two maps and some counters), so moving it costs the core crate no
//! dependency at all.
//!
//! `dux-web` re-exports it, so every socket handler, route and test there keeps
//! the path it always used.
//!
//! ## Who reads it
//!
//! The per-PTY socket handlers gate stdin and resizes through it. Two web-side
//! consumers read it outside those handlers, and both are deliberately narrow:
//! the file-drop route's courtesy check, and the engine actor's spine overlay
//! ([`Self::input_owners_snapshot`] plus [`Self::ownership_generation`]), which
//! publishes the owning connection id on the shared spine so every client,
//! including one with no PTY socket attached, can tell that another connection
//! is driving an agent. The terminal UI reads it through the background-serve
//! seam, holding a connection id of its own.

/// One recorded owner: the connection id that drives the pty, plus the raw
/// `User-Agent` that connection presented at its upgrade. The device rides in
/// the SAME map entry as the id, written at claim time and removed with the
/// entry on release, so [`PtySizeOwners::current_owner`] can hand the handshake
/// the owner's device label under the same lock acquisition as the id and the
/// epoch. Without it the label existed only as a local in the claiming socket's
/// task, so only the `pty.owner` broadcast could name the device, and a watcher
/// that merely attached (which broadcasts nothing) could not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerRecord {
    pub conn_id: u64,
    /// The owning connection's raw `User-Agent`, already length-bounded by the
    /// capture at the upgrade; `None` when it sent none.
    pub device: Option<String>,
}

/// The owner map plus the monotonic ownership epoch, guarded together by ONE std
/// Mutex so a fresh epoch is assigned in the SAME critical section that records a
/// new owner. Bumping the epoch under the lock that serializes every owner write
/// makes epochs monotonic in TRUE claim order even when two connections claim
/// concurrently, so the `pty.owner` broadcast (emitted after the lock releases, and
/// therefore freely reorderable by the runtime) can be deduped by epoch on the
/// client without confusing which claim actually won (see `ptyOwnership.ts`).
#[derive(Default)]
pub struct OwnersState {
    /// pty id -> the connection that currently owns sizing+input.
    pub map: std::collections::HashMap<String, OwnerRecord>,
    /// Bumped on every ownership CHANGE, a release included; the value handed to
    /// the caller and stamped onto the emitted `pty.owner` so clients converge on
    /// the latest claim regardless of broadcast arrival order. Never decreases
    /// within a process.
    pub epoch: u64,
    /// Bumped on every MUTATION of `map` — a claim handover, a first-writer
    /// claim, and a release that actually removed an entry. Distinct from
    /// `epoch` in what it FEEDS rather than in when it moves: this is the spine
    /// check's cheap "did ownership change" gate, read by
    /// [`PtySizeOwners::ownership_generation`], while the epoch travels on the
    /// wire to order client-side arrivals. The fingerprint compare downstream
    /// remains the precise emit gate.
    pub generation: u64,
    /// Per-pty grid sequence, bumped on every resize that APPLIES, inside the
    /// same critical section that enqueues the resize to the engine actor. The
    /// applies come out of the actor in claim order (see
    /// [`PtySizeOwners::claim_for_resize`]), but each socket task publishes its
    /// grid broadcast AFTER this lock releases, so two sockets' announcements
    /// of two ordered applies can reach the bus inverted (A applies G2, B takes
    /// over and applies G3, B's publish lands first, A's stale G2 becomes every
    /// viewer's last word). Stamping the seq under the lock gives every
    /// receiver a total order to drop stale announcements by, exactly as
    /// `epoch` does for `pty.owner`. Keyed per pty because the broadcasts are
    /// filtered per pty; never decreases within a process.
    pub grid_seq: std::collections::HashMap<String, u64>,
    /// Per-pty high-water mark of the seq that actually REACHED the child, the
    /// other half of [`PtySizeOwners::accept_grid_apply`]. Distinct from
    /// `grid_seq`, which counts what was stamped: the two differ for exactly as
    /// long as a stamped resize is still queued somewhere, and that window is
    /// where the inversion lives.
    pub applied_seq: std::collections::HashMap<String, u64>,
}

/// Tracks which connection currently owns sizing+input for each PTY, keyed by
/// pty id (the tab id for an agent PTY, which is the session id for the
/// session-slot tab, and the terminal id for a companion). Shared between every
/// per-PTY socket, the engine actor loop and, while a background server runs
/// behind the terminal UI, that terminal UI. The web layer's
/// `build_actor_channels` is what constructs one, once per serve.
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
pub struct PtySizeOwners {
    pub owners: std::sync::Mutex<OwnersState>,
}

/// The source of every connection id in the process.
///
/// PROCESS-global, not registry-global, and that distinction is the whole point.
/// A registry is built once per serve (`build_actor_channels` constructs it), and
/// the background-server toggle can build several of them in one run. A
/// per-registry counter therefore started again at zero on every cycle, and the
/// ghost self-succession rule (a returning owner recognising "this pane's
/// previous, dead connection id was mine") compares raw ids: a second cycle's
/// connection 0 would answer to a first cycle's connection 0 and take a pty away
/// from whichever device is actually driving it. Ids never repeat while the
/// process lives, so nothing can be mistaken for a stale self.
static NEXT_CONN_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Outcome of [`PtySizeOwners::may_write`]: whether the connection may forward its
/// stdin to the PTY (`allowed`), whether the check itself NEWLY claimed an unowned
/// PTY (`claimed_new`) so the caller emits exactly one `pty.owner` handover for that
/// uncontested first write, and the ownership `epoch` assigned for that new claim
/// (`Some` iff `claimed_new`) so the emitted handover carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteClaim {
    pub allowed: bool,
    pub claimed_new: bool,
    pub epoch: Option<u64>,
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
///
/// `seq` is `Some` exactly when `apply` is: the per-pty grid sequence stamped
/// in the SAME critical section that enqueued the resize, carried onto the
/// grid broadcast so receivers can drop a stale announcement that the runtime
/// reordered after the lock released (see [`OwnersState::grid_seq`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeClaim {
    pub apply: bool,
    pub epoch: Option<u64>,
    pub seq: Option<u64>,
}

impl PtySizeOwners {
    /// Allocate a process-unique id for a freshly attached PTY socket, used to
    /// compare against the recorded owner. Drawn from [`NEXT_CONN_ID`], so ids
    /// stay unique across serve cycles rather than only within one registry.
    pub fn next_conn_id(&self) -> u64 {
        NEXT_CONN_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
    ///
    /// The owner's DEVICE label (its captured `User-Agent`) travels in the same
    /// snapshot, for the same reason the epoch does: it is what lets the
    /// take-over card of a client that merely attached name the driving device,
    /// because a mere attach emits no `pty.owner` for that client to hear.
    pub fn current_owner(&self, pty_id: &str) -> (Option<u64>, u64, Option<String>) {
        let owners = self.owners.lock().unwrap();
        let record = owners.map.get(pty_id);
        (
            record.map(|r| r.conn_id),
            owners.epoch,
            record.and_then(|r| r.device.clone()),
        )
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
    /// the web layer's `EngineHandle::resize_pty` is a separate `try_send`
    /// into the engine actor's queue with nothing binding the two orders.
    /// Enqueuing the resize INSIDE this critical section binds them: `try_send`
    /// never blocks and the actor drains its queue in order, so the resizes come
    /// out in claim order and the epoch winner's geometry is the one that lands
    /// last, always. The lock is therefore held for a fixed, tiny window and
    /// never across an await. This mirrors the precedent set by
    /// [`Self::may_write`], which likewise resolves the stdin gate under the lock
    /// rather than checking and then writing.
    ///
    /// `device` is the claiming connection's captured `User-Agent`, recorded
    /// with the owner id on a claim so [`Self::current_owner`] can name the
    /// device on later handshakes; it is ignored on every non-claiming outcome.
    ///
    /// `apply_resize` is handed the seq stamped for this resize, because a caller
    /// that merely ENQUEUES the resize has to carry it to wherever the resize is
    /// finally applied: that apply site offers the seq to
    /// [`Self::accept_grid_apply`] and drops the resize if a later claim's
    /// geometry has already reached the child.
    pub fn claim_for_resize(
        &self,
        pty_id: &str,
        conn_id: u64,
        takeover: bool,
        device: Option<&str>,
        apply_resize: impl FnOnce(u64),
    ) -> ResizeClaim {
        let mut owners = self.owners.lock().unwrap();
        let mut outcome = match owners.map.get(pty_id) {
            Some(record) if record.conn_id == conn_id => ResizeClaim {
                apply: true,
                epoch: None,
                seq: None,
            },
            Some(_) if !takeover => ResizeClaim {
                apply: false,
                epoch: None,
                seq: None,
            },
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
            // Stamp the grid sequence in the SAME critical section that
            // enqueues the resize, so the seq order IS the apply order. The
            // caller's grid broadcast happens after this lock releases and is
            // freely reorderable by the runtime; the seq is what lets every
            // receiver drop an announcement that arrives behind a newer one.
            let seq = owners.grid_seq.entry(pty_id.to_string()).or_insert(0);
            *seq += 1;
            let seq = *seq;
            outcome.seq = Some(seq);
            apply_resize(seq);
        }
        outcome
    }

    /// Hand `pty_id` to `conn_id` unconditionally, the way an explicit take-over
    /// does, and report the handover epoch (`None` when it already owned it).
    /// A thin spelling of [`Self::claim_for_resize`] with `takeover: true` and no
    /// resize to apply, so there is exactly one implementation of "record a new
    /// owner". Used by the test fixture that gives the file-drop courtesy check
    /// something to say, and by the claim-table tests.
    ///
    /// Not `#[cfg(test)]`, unlike the shape it had while this type lived in the
    /// web crate: the tests that need it are now in three crates, and a cfg that
    /// only sees this one's test build would hide it from all of them.
    pub fn claim(&self, pty_id: &str, conn_id: u64) -> Option<u64> {
        self.claim_for_resize(pty_id, conn_id, true, None, |_| {})
            .epoch
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
    pub fn is_owner(&self, pty_id: &str, conn_id: u64) -> bool {
        self.owners
            .lock()
            .unwrap()
            .map
            .get(pty_id)
            .is_some_and(|record| record.conn_id == conn_id)
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
    ///
    /// `device` is the writing connection's captured `User-Agent`, recorded with
    /// the owner id when the write newly claims an unowned pty (so later
    /// handshakes can name the device, exactly as a resize claim records it);
    /// it is ignored on every other outcome.
    pub fn may_write(&self, pty_id: &str, conn_id: u64, device: Option<&str>) -> WriteClaim {
        let mut owners = self.owners.lock().unwrap();
        match owners.map.get(pty_id) {
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

    /// Release every pty `conn_id` still owns, and report each one with the epoch
    /// its release was stamped with, so the caller can announce them all.
    ///
    /// A browser connection owns at most the one pty its socket is attached to, so
    /// it never needed this. The terminal UI is a single participant that can end
    /// up driving several ptys at once (it types into one agent, then another),
    /// and it lets go of all of them at the same moment: the background server
    /// stops, the terminal is handed to the flip, or dux quits. Doing that in one
    /// critical section rather than a loop of `release` calls keeps the epochs
    /// consecutive and means no browser can claim one of them halfway through the
    /// sweep and have its claim silently released underneath it.
    ///
    /// The order of the returned pairs follows the epochs, which is the order the
    /// announcements have to be published in.
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

    /// THE ONE APPLY ORDER: may a resize stamped with `seq` still reach the
    /// child of `pty_id`?
    ///
    /// Every surface stamps its resize under this lock, in true claim order, but
    /// the surfaces do not APPLY at the same moment. A browser's resize is
    /// enqueued to the engine actor and lands whenever that queue is drained; the
    /// terminal UI holds the engine and applies at once. So a resize stamped
    /// FIRST can reach the child LAST, leaving the pty sized for a connection
    /// that no longer owns it while the owner believes the child already knows
    /// its geometry. Nothing corrects that afterwards: the owner has no reason to
    /// resend a size it never changed.
    ///
    /// So each apply site offers its seq here immediately before touching the
    /// child, and a seq that is not strictly newer than the last one that landed
    /// is dropped. Two apply sites, one order, decided in one place.
    ///
    /// Dropping is the right answer rather than a loss: a dropped resize is by
    /// definition superseded by one that already landed, and the viewer whose
    /// resize was dropped learns the real grid from the handshake and the grid
    /// broadcast, both of which report what the child was actually told.
    ///
    /// In a web-only process this never refuses anything, because there is one
    /// apply site and the actor drains its queue in order. It earns its keep only
    /// while the terminal UI is a participant too.
    pub fn accept_grid_apply(&self, pty_id: &str, seq: u64) -> bool {
        let mut owners = self.owners.lock().unwrap();
        let landed = owners.applied_seq.entry(pty_id.to_string()).or_insert(0);
        if seq <= *landed {
            return false;
        }
        *landed = seq;
        true
    }

    /// The per-pty grid sequence as of now: the seq of the last APPLIED resize,
    /// or 0 before any. Read once per PTY-socket handshake, BEFORE the
    /// actor-queued grid read, which makes it a valid lower bound for the grid
    /// that read returns: a resize stamped at or below this value was enqueued
    /// (inside the same critical section that stamped it) before this call, and
    /// the actor drains in order, so the handshake's grid already reflects it.
    /// The client seeds its last-seen seq from this value, so a stale broadcast
    /// that was still buffered on the socket when the handshake was sent can
    /// never regress the grid after it.
    pub fn grid_seq(&self, pty_id: &str) -> u64 {
        self.owners
            .lock()
            .unwrap()
            .grid_seq
            .get(pty_id)
            .copied()
            .unwrap_or(0)
    }

    /// The mutation counter for the owner map, read by the engine actor's spine
    /// check as its cheap "ownership might have changed" gate signal, exactly
    /// like `mutation_version` and `streaming_version`. See
    /// [`OwnersState::generation`] for why this is not `epoch`.
    pub fn ownership_generation(&self) -> u64 {
        self.owners.lock().unwrap().generation
    }

    /// A point-in-time copy of the owner map (pty id -> owning connection id),
    /// taken by the spine check when it actually runs a fingerprint compare so
    /// the overlay stamps a CONSISTENT set of owners onto one spine build. A
    /// clone rather than a borrow: the map is small (one entry per driven PTY)
    /// and the lock must not be held across the spine projection.
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
        let applied = std::cell::Cell::new(false);
        let outcome = owners.claim_for_resize(pty, conn, takeover, None, |_| applied.set(true));
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
        let first = owners.claim_for_resize("p", a, false, None, |_| {
            applied.borrow_mut().push((a, 80));
        });
        let second = owners.claim_for_resize("p", b, true, None, |_| {
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
            "the accessor reports the last APPLIED seq"
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
        let queued = owners.claim_for_resize("p", browser, false, None, |_| {});
        let queued_seq = queued.seq.expect("the claim applied, so it stamped a seq");

        // The terminal UI takes over at 100 columns and applies immediately.
        let direct = owners.claim_for_resize("p", tui, true, None, |_| {});
        let direct_seq = direct.seq.expect("the take-over applied");
        assert!(
            owners.accept_grid_apply("p", direct_seq),
            "the newest stamped resize is the one that may reach the child"
        );
        applied.borrow_mut().push((tui, 100));

        // Now the browser's queued resize is drained. It must not land: the
        // terminal UI owns the pty and its geometry is already on the child.
        assert!(
            !owners.accept_grid_apply("p", queued_seq),
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
            .claim_for_resize("p", browser, true, None, |_| {})
            .seq
            .expect("the take-over applied");
        assert!(owners.accept_grid_apply("p", again));
    }

    /// The gate is per pty, and it never refuses the very first apply.
    #[test]
    fn the_apply_gate_starts_open_and_counts_per_pty() {
        let owners = PtySizeOwners::default();
        let conn = owners.next_conn_id();

        let first = claim_resize(&owners, "p", conn, false).0.seq.unwrap();
        assert!(owners.accept_grid_apply("p", first), "nothing applied yet");
        assert!(
            !owners.accept_grid_apply("p", first),
            "the same seq offered twice is a duplicate, not a newer geometry"
        );

        // Another pty is stamped and gated on its own counter, exactly as the
        // broadcasts are filtered per pty.
        let other = claim_resize(&owners, "q", conn, false).0.seq.unwrap();
        assert!(owners.accept_grid_apply("q", other));
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
        let claim = owners.claim_for_resize("p", a, false, Some("Desktop UA"), |_| {});
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
        let takeover = owners.claim_for_resize("p", b, true, None, |_| {});
        assert!(takeover.epoch.is_some());
        assert_eq!(
            owners.current_owner("p"),
            (Some(b), takeover.epoch.unwrap(), None)
        );

        // The release removes the device with the entry.
        let cleared = owners.release("p", b).expect("the owner's release");
        assert_eq!(owners.current_owner("p"), (None, cleared, None));

        // A first-writer claim records the device too, exactly like a resize claim.
        let write = owners.may_write("p", a, Some("Phone UA"));
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

        let claim = owners.may_write("s2", a, None);
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
        assert!(owners.may_write("s1", a, None).allowed, "owner keystroke");
        assert!(
            !owners.may_write("s1", b, None).allowed,
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
}
