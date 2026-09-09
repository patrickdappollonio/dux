// Client half of the per-PTY owner model mirrored from the server's
// `PtySizeOwners`: one owner drives a shared PTY's size and typing, everyone
// else watches, and every handover arrives as a `pty.owner` broadcast carrying
// the claimer's connection id.

// A foregrounded tab claims on attach by sending its size; a backgrounded one
// attaches silently. Read at call time, and foreground when there is no
// `document`, so a claim is never silently suppressed.
export function isForeground(): boolean {
  return typeof document === "undefined"
    ? true
    : document.visibilityState === "visible"
}

/// What the `connected` handshake said about who drives this pty:
/// a connection id, `null` for "nobody", or `undefined` for a server that does
/// not answer the question (the key was absent).
export type HandshakeOwner = string | null | undefined

// Seed the ownership verdict from the server's `connected` answer. A refused
// claim is silent, so the foreground guess alone would leave a watcher rendering
// typing surfaces whose keystrokes are dropped; it survives only for an unowned
// pty. The rule order is the contract:
//   1. An armed take-over wins: its first resize frame carries the flag.
//   2. A superseded handshake keeps the prior verdict. Handshake and `pty.owner`
//      ride different sockets with no ordering between them, and the stale-null
//      direction emits no correcting event.
//   3. An absent owner key is an old server that grants any claim: use foreground.
//   4. `null` is nobody driving: foreground decides, and only here.
//   5. Otherwise compare ids; anything but our own means we are a watcher.
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

// The take-over device name the pane should store, seeded from the same
// handshake as the verdict: an attach hears no `pty.owner`, so `owner_device`
// (the owner's raw `User-Agent`) is a watcher's only word on who drives. The
// rules mirror the verdict seed's:
//   - This pane owns the pty: null, an owner never names another device.
//   - Superseded handshake: keep the prior name, in either direction.
//   - No owner id (absent key or unowned pty): null, so the card falls back to
//     its generic title rather than a stale name.
//   - A foreign owner: its device, or null when it sent no User-Agent.
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

// Whether a `connected` handshake's owner snapshot was overtaken by an already
// applied `pty.owner`: true only when both epochs are known and the applied one
// is strictly newer, so equal epochs and a missing epoch on either side both
// seed normally. The one implementation of the comparison, so the verdict seed
// and the ownership machine's `ownerPresent` side effect cannot drift.
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

// Ownership after a `pty.owner` handover, by id and never by timing: a guess at
// whether the event is our own claim echoing back inverts when two devices claim
// at once and broadcast order flips. A missing id on either side means "not us",
// so a client observes rather than wrongly assuming control.
export function isOwnerAfterHandover(
  eventOwnerId: string | undefined,
  myConnId: string | null,
): boolean {
  return myConnId !== null && eventOwnerId === myConnId
}

// `pty.owner` fan-out: the store's `/ws/events` handler calls `notifyPtyOwner`
// and each mounted terminal view listens for its own pty id. Kept out of the
// store so the view depends on a leaf module, as `ptySocket.ts` does.
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

// The highest `pty.owner` epoch already applied per pty id. The server assigns
// the epoch under its owners lock but broadcasts after releasing it, so epoch
// order is the true claim order and arrival order is not; dropping anything not
// strictly newer converges every client on the latest claim.
const lastEpochByPty = new Map<string, number>()

// Reset the per-pty epoch high-water marks, called on every events reconnect:
// the server's counter restarts at zero on a process restart, and a client
// holding a high mark would ignore every post-restart handover as stale.
// It clears these marks and nothing else. Firing on reconnects that follow no
// restart is safe only for this state, where a forgotten mark can merely accept
// a handover; state that must survive a dropped socket lives in `serverRun.ts`.
export function resetPtyOwnerEpochs(): void {
  lastEpochByPty.clear()
}

// The newest `pty.owner` epoch already applied for `ptyId`, or undefined when
// none has been. Read by the handshake seed to tell a stale `connected` owner
// snapshot from a fresh one; see `seedVerdictFromConnected` rule 2.
export function appliedPtyOwnerEpoch(ptyId: string): number | undefined {
  return lastEpochByPty.get(ptyId)
}

export function notifyPtyOwner(
  ptyId: string,
  ownerId: string | undefined,
  epoch?: number,
  device?: string,
): void {
  // An absent epoch (older server, or a non-`pty.owner` caller) is always
  // delivered and never recorded, so mixed versions degrade to last-arrival
  // ordering rather than silently dropping events.
  if (typeof epoch === "number") {
    const last = lastEpochByPty.get(ptyId)
    if (last !== undefined && epoch <= last) return
    lastEpochByPty.set(ptyId, epoch)
  }
  // Snapshot so a listener that unsubscribes during dispatch can't perturb the
  // live iteration.
  for (const cb of [...ptyOwnerListeners]) cb(ptyId, ownerId, device)
}
