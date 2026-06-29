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
type PtyOwnerListener = (ptyId: string, ownerId: string | undefined) => void
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

export function notifyPtyOwner(
  ptyId: string,
  ownerId: string | undefined,
  epoch?: number,
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
  for (const cb of [...ptyOwnerListeners]) cb(ptyId, ownerId)
}
