// Per-PTY active-owner model (client side). A PTY (an agent's provider or a
// companion terminal) is SHARED across every connected device, but only ONE of
// them — the "owner" — drives its size and may type into it. The others render a
// read-only take-over placeholder. This mirrors the server's `PtySizeOwners`
// (`crates/dux-web/src/server.rs`): a connection claims ownership by sending a
// size frame (most-recent claim wins) or by being the first writer of an unowned
// PTY, and the server broadcasts a `pty.owner` signal carrying the claimer's
// connection id on every handover.
//
// This module holds the small, pure pieces of that model so they are unit-
// testable without rendering xterm (which needs a DOM/canvas harness the web
// tests deliberately avoid): the foreground check that decides whether a fresh
// attach claims, the definitive owner comparison that reads a `pty.owner` against
// this client's own PTY-socket connection id, and the `pty.owner` event fan-out
// the store pushes into.

// A foregrounded tab claims ownership on attach by sending its size; a
// backgrounded tab attaches as a silent observer and sends nothing. This is the
// decision the terminal view seeds its initial owner state from (and gates its
// size sends on). Read at call time so tests can stub `document.visibilityState`
// per case, and treated as foreground when there is no `document` (non-DOM
// contexts) so a claim is never silently suppressed.
export function isForeground(): boolean {
  return typeof document === "undefined"
    ? true
    : document.visibilityState === "visible"
}

/// What the `connected` handshake said about who drives this pty:
/// a connection id, `null` for "nobody", or `undefined` for a server that does
/// not answer the question (the key was absent).
export type HandshakeOwner = string | null | undefined

// SEED the ownership verdict from the server's own answer, at the moment the
// `connected` frame lands. This is the correction that makes "a plain resize
// claims only an unowned pty" safe to ship.
//
// The foreground guess alone used to be enough because a foregrounded attach
// really did take the pty (the server granted any resize as a claim). Now it
// does not, and a refused claim is SILENT by design, so a phone opening an agent
// its owner's desktop is driving would guess "foregrounded, therefore mine",
// render its typing surfaces, and have every keystroke dropped server-side with
// no card ever explaining why. The handshake closes that hole: the foreground
// check survives only as the decision to claim an UNOWNED pty.
//
// The order of the rules is the whole content:
//   1. An ARMED take-over outranks everything. The client is deliberately
//      claiming, its first resize frame will carry the flag, and the server will
//      grant it; demoting it here would flash the card back over a pane the user
//      has just taken.
//   2. A SUPERSEDED handshake defers. The handshake rides the PTY socket while
//      `pty.owner` rides the events socket, two TCP connections with no
//      ordering between them, so a fresh `pty.owner{owner:B}` can be applied
//      before a STALE `connected{owner:null}` lands. The server stamps both
//      with the same monotonic epoch counter; when the newest `pty.owner`
//      already applied for this pty is strictly newer than the handshake's
//      snapshot, the seed keeps the verdict that newer event wrote instead of
//      resurrecting the stale answer, because the stale-null direction emits no
//      correcting event, ever.
//   3. An ABSENT owner key means an older server that still grants any claim, so
//      fall back to the foreground guess rather than assuming anything. (An old
//      server sends no epoch either, so rule 2 never fires for it.)
//   4. `null` means nobody is driving: the foreground guess decides, exactly as
//      it always did, and this is now the ONLY case it decides.
//   5. Otherwise, compare ids. Equal is ours (unreachable at a fresh handshake,
//      since the id is minted for this socket, but the comparison is the honest
//      rule rather than an assumption about allocation order); anything else
//      means another device drives it and this client is a watcher.
export function seedVerdictFromConnected(input: {
  owner: HandshakeOwner
  myConnId: string
  foreground: boolean
  takeoverArmed: boolean
  /// The `owner_epoch` stamped on the `connected` frame; undefined on an old
  /// server (which then omitted `owner` too).
  handshakeEpoch?: number
  /// The newest `pty.owner` epoch already applied for this pty (the client's
  /// per-pty dedup high-water mark); undefined when none has been applied.
  appliedEpoch?: number
  /// The verdict standing when the handshake landed, returned unchanged when
  /// the handshake is superseded by a newer applied `pty.owner`.
  priorVerdict?: boolean
}): boolean {
  if (input.takeoverArmed) return true
  if (handshakeSuperseded(input.handshakeEpoch, input.appliedEpoch)) {
    return input.priorVerdict ?? false
  }
  if (input.owner === undefined) return input.foreground
  if (input.owner === null) return input.foreground
  return input.owner === input.myConnId
}

// Whether a `connected` handshake's owner snapshot has been overtaken by a
// `pty.owner` already applied for the same pty: true only when BOTH epochs are
// known and the applied one is strictly newer. Equal epochs mean the handshake
// snapshot was taken at (or after) that claim, so it is fresh; a missing epoch
// on either side means there is nothing to order by (an old server, or no
// handover applied yet) and the handshake seeds normally. The ONE
// implementation of the comparison: `seedVerdictFromConnected` gates the
// verdict on it, and the ownership machine gates its `ownerPresent` side
// effect on the same answer so the two cannot drift.
export function handshakeSuperseded(
  handshakeEpoch: number | undefined,
  appliedEpoch: number | undefined,
): boolean {
  return (
    typeof handshakeEpoch === "number" &&
    typeof appliedEpoch === "number" &&
    appliedEpoch > handshakeEpoch
  )
}

// Decide ownership after a `pty.owner` handover by comparing the claimer's
// connection id (the event's `owner` field) against THIS client's own PTY-socket
// connection id (received as the socket's first `connected` frame). The comparison
// is DEFINITIVE: an equal id means this client made the claim and is the owner; a
// different id means another device took over, so this view shows the read-only
// placeholder. A missing id on either side (our `connected` frame has not arrived,
// or the event carried no owner) is treated as "not us" — the safe default of
// observing rather than wrongly assuming control.
//
// This replaces the old timing/echo-counting heuristic, which guessed whether an
// event was our own claim echoing back. That guess inverted when two devices
// claimed in the same instant and broadcast order flipped, leaving BOTH devices on
// the placeholder while the server held a real owner. Comparing stable ids removes
// the guess and the race: every client converges on the same final `pty.owner`.
export function isOwnerAfterHandover(
  eventOwnerId: string | undefined,
  myConnId: string | null,
): boolean {
  return myConnId !== null && eventOwnerId === myConnId
}

// `pty.owner` fan-out. The store's single `/ws/events` handler receives the
// signal and calls `notifyPtyOwner(ptyId, ownerId)`; each mounted terminal view
// registers an `onPtyOwner` listener and reacts only to its own pty id, comparing
// `ownerId` to its own PTY-socket connection id. Kept here (not in the store) so
// the terminal view depends on a small leaf module rather than the store, matching
// the `setActivePtySocket` singleton pattern in `ptySocket.ts`.
type PtyOwnerListener = (
  ptyId: string,
  ownerId: string | undefined,
  device?: string,
) => void
const ptyOwnerListeners = new Set<PtyOwnerListener>()

export function onPtyOwner(cb: PtyOwnerListener): () => void {
  ptyOwnerListeners.add(cb)
  return () => {
    ptyOwnerListeners.delete(cb)
  }
}

// The highest `pty.owner` epoch already applied per pty id. The server stamps every
// ownership handover with a monotonic epoch assigned UNDER its owners lock, so the
// epoch order is the TRUE claim order even though the broadcast is emitted after the
// lock releases and the runtime may reorder two near-simultaneous broadcasts.
// Dropping any handover whose epoch is not strictly newer than the last applied for
// that pty makes the client converge on the latest claim regardless of arrival
// order, closing the two-device simultaneous-claim race that reordering would
// otherwise reopen (the map could end on owner=A while a client saw owner=B last).
const lastEpochByPty = new Map<string, number>()

// Reset the per-pty epoch high-water marks. The server's epoch counter restarts at
// zero on a process restart, so without this a client that had seen a high epoch
// would wrongly ignore every post-restart handover as "stale". Call this when the
// events socket reconnects (a reconnect is the only way a restarted server's epochs
// reach this client). Exported primarily for that wiring and for test isolation.
export function resetPtyOwnerEpochs(): void {
  lastEpochByPty.clear()
}

// The newest `pty.owner` epoch already applied for `ptyId`, or undefined when
// none has been. Read by the handshake seed so it can tell a stale `connected`
// owner snapshot (taken before a handover this client has already applied)
// from a fresh one, and defer to the newer verdict; see
// `seedVerdictFromConnected` rule 2.
export function appliedPtyOwnerEpoch(ptyId: string): number | undefined {
  return lastEpochByPty.get(ptyId)
}

export function notifyPtyOwner(
  ptyId: string,
  ownerId: string | undefined,
  epoch?: number,
  device?: string,
): void {
  // Epoch-ordered dedup: ignore a handover that is not strictly newer than the
  // newest already applied for this pty, so a reordered (older) broadcast cannot
  // override a newer claim. An absent epoch (older server, or a non-`pty.owner`
  // caller) is always delivered and never recorded, so mixed versions degrade to
  // the prior last-arrival behavior rather than silently dropping events.
  if (typeof epoch === "number") {
    const last = lastEpochByPty.get(ptyId)
    if (last !== undefined && epoch <= last) return
    lastEpochByPty.set(ptyId, epoch)
  }
  // Snapshot so a listener that unsubscribes during dispatch can't perturb the
  // live iteration.
  for (const cb of [...ptyOwnerListeners]) cb(ptyId, ownerId, device)
}
