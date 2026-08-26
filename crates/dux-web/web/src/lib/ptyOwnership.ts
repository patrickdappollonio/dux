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
// The foreground guess alone is not enough: the server does not grant a resize
// against an owned pty, and a refused claim is SILENT by design, so a phone
// opening an agent its owner's desktop is driving would guess "foregrounded,
// therefore mine", render its typing surfaces, and have every keystroke dropped
// with no card explaining why. The handshake closes that hole: the foreground
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

// SEED the other device's NAME from the same `connected` handshake the verdict
// seeds from. The handshake's `owner_device` is the owner's raw `User-Agent`,
// recorded server-side at claim time and read under the same lock as `owner`,
// and it exists because a mere attach hears no `pty.owner` broadcast at all:
// under attach-never-steals the handshake is a watcher's ONLY word on who
// drives, so without this rule its card could only say "Active on another
// device". Returns what the pane should store as the take-over device string.
//
// The rules mirror the verdict seed's, one for one:
//   - This pane OWNS the pty: null. An owning pane never names another device.
//   - The handshake is SUPERSEDED by a newer applied `pty.owner`: keep the
//     prior name, exactly as the verdict keeps the prior verdict. The newer
//     event's name (or its absence) already stands; a stale snapshot must not
//     overwrite it in either direction.
//   - No owner id on the handshake (an old server's absent key, or an unowned
//     pty's null): null. There is no device to name, and an old server that
//     cannot answer falls back to the generic title rather than a stale name.
//   - Otherwise the handshake names a foreign owner: its device, or null when
//     that connection sent no User-Agent.
export function seedDeviceFromConnected(input: {
  /// The verdict `seedVerdictFromConnected` returned for this same handshake.
  mine: boolean
  /// `handshakeSuperseded` for this same handshake, computed once by the caller
  /// so the name and the verdict cannot disagree about staleness.
  superseded: boolean
  owner: HandshakeOwner
  /// The handshake's `owner_device`; undefined when absent (an old server, an
  /// unowned pty, or an owner that sent no User-Agent).
  ownerDevice: string | undefined
  /// The name standing when the handshake landed.
  priorDevice: string | null
}): string | null {
  if (input.mine) return null
  if (input.superseded) return input.priorDevice
  if (typeof input.owner !== "string") return null
  return input.ownerDevice ?? null
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
// Ids, never timing: guessing whether an event is our own claim echoing back
// inverts when two devices claim in the same instant and broadcast order flips,
// leaving BOTH on the placeholder while the server holds a real owner.
// Comparing stable ids makes every client converge on the same final
// `pty.owner`.
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
  for (const cb of [...epochResetListeners]) cb()
}

// Listeners for that same reset. A restarted server restarts EVERY
// process-global counter, connection ids included, so anything a client
// remembers about the old run's identifiers is not merely stale but actively
// wrong: an id from the previous run can be minted again for a different
// connection. The epoch high-water marks are one such memory and a pane's set of
// its own past connection ids is another, so they are retired on the one signal.
const epochResetListeners = new Set<() => void>()

export function onPtyOwnerEpochsReset(cb: () => void): () => void {
  epochResetListeners.add(cb)
  return () => {
    epochResetListeners.delete(cb)
  }
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
