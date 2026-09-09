// A shared PTY has one driver. Plain attaches never steal; take-over arms an
// intent and reconnects so the claim rides a fresh attach and replay.
//
//   OWNER     input and resize writes are enabled.
//   OBSERVER  another connection (or none while backgrounded) owns the PTY.
//   CLAIMING  a take-over reconnect is waiting to send its flagged first resize.
//   LOST      a terminal socket failure suppresses the stale local verdict.
//
// Handshakes and `pty.owner` events are authoritative. Losing ownership remains
// sticky until an explicit take-over or confirmed self-succession. The verdict
// channel updates synchronous write gates and React state together.
import { useEffect, useMemo, useRef, useState } from "react"

import { deviceLabel } from "@/lib/deviceLabel"
import type { PtySocket } from "@/lib/ptySocket"
import {
  appliedPtyOwnerEpoch,
  handshakeSuperseded,
  isForeground,
  isOwnerAfterHandover,
  onPtyOwner,
  seedDeviceFromConnected,
  seedVerdictFromConnected,
  type HandshakeOwner,
} from "@/lib/ptyOwnership"
import {
  currentRunStamp,
  onServerRunChanged,
  runIdentityConfirmedAs,
} from "@/lib/serverRun"
import { noteAgentPtyOwnership } from "@/lib/store"
import type { ConnState } from "@/lib/types"

import type {
  ConnectionIdentity,
  OwnershipVerdict,
  TakeoverIntent,
} from "./channels"

export type TerminalOwnershipDeps = {
  /// The pty id: the session id for an agent, the terminal id for a companion.
  id: string
  kind: "agent" | "terminal"
  /// The EVENTS socket's state, which decides whether the other device's name
  /// can still be trusted.
  conn: ConnState
  ptyRef: { current: PtySocket | null }
  /// Who the spine says drives this pty, refetched on every events-socket open.
  /// `null` when the server says nobody, `undefined` when it has not answered.
  /// The only thing that can correct a device name kept across an outage.
  spineInputOwner?: string | null
  /// The pane's reconnect cue, raised by hand: the take-over bounce closes the
  /// socket deliberately and so fires no `onReconnecting` of its own, and the
  /// window would otherwise read as a dead terminal.
  setReconnecting: (value: boolean) => void
}

export type TerminalOwnership = {
  /// The rendered verdict.
  isOwner: boolean
  /// The verdict channel, for the lifecycle's stable closures.
  ownership: OwnershipVerdict
  /// This socket's connection id, owned by the lifecycle's attach wiring and
  /// read here for the handover comparison.
  connId: ConnectionIdentity
  /// The take-over intent, armed by `takeOver` and consumed by the one
  /// confirmed resize write in the lifecycle.
  takeoverIntent: TakeoverIntent
  /// Re-seed the verdict from the `connected` handshake. `ownerEpoch` is the
  /// handshake's `owner_epoch`, which lets the seed defer to a strictly newer
  /// `pty.owner` already applied; `ownerDevice` is its `owner_device`, which
  /// names the card's device for a watcher that will hear no `pty.owner`
  /// broadcast. Both are undefined on a server that sends neither.
  seedFromConnected: (
    myConnId: string,
    owner: HandshakeOwner,
    ownerEpoch?: number,
    ownerDevice?: string,
  ) => void
  /// A human label for the device that took over ("Chrome on macOS"), or null
  /// when the other device's `User-Agent` was absent, unrecognized, or stale.
  takeoverLabel: string | null
  /// Whether SOMEBODY drives this pty right now, as far as this client knows.
  /// False means the driver disconnected and nobody has claimed it since, which
  /// the card says out loud rather than claiming a device is active. Only
  /// meaningful while `isOwner` is false.
  ownerPresent: boolean
  /// Whether the `connected` handshake has answered the ownership question at
  /// least once on this mount. Until it has, `isOwner` is only the foreground
  /// GUESS, which is not a good enough reason to raise a soft keyboard.
  handshakeSeen: boolean
  /// Whether this socket has given up for good.
  connectionLost: boolean
  setConnectionLost: (value: boolean) => void
  /// Feed the PTY socket connection state in. Owns the LOST state and the
  /// take-over intent lifetime; see the function.
  notePtyConn: (state: ConnState) => void
  takeOver: () => void
}

export function useTerminalOwnership(
  deps: TerminalOwnershipDeps,
): TerminalOwnership {
  const {
    id,
    kind,
    conn,
    spineInputOwner,
    ptyRef,
    setReconnecting,
  } = deps

  // The foreground guess lasts only until the server's connected handshake.
  // No-document contexts count as foreground so a claim is never suppressed.
  const [isOwner, setIsOwner] = useState(isForeground)
  const isOwnerRef = useRef(isOwner)
  const ownership = useMemo<OwnershipVerdict>(
    () => ({
      read: () => isOwnerRef.current,
      write: (mine) => {
        isOwnerRef.current = mine
        setIsOwner(mine)
      },
    }),
    [],
  )

  const myConnIdRef = useRef<string | null>(null)
  // Every id this pane has ever held, not merely the last: a returning owner
  // recognises its own dead connection in the next handshake (see the
  // self-succession rule in `seedFromConnected`), and a flapping radio can
  // produce two handshakes where the second still names the connection from
  // before the first. Each id is stamped with the server run it was learned
  // under, because a restarted server mints ids from zero again and another
  // device's fresh id can equal one of ours (see `serverRun.ts`).
  const heldConnIdsRef = useRef<Map<string, number>>(new Map())
  // Ghosts do not survive a server restart: ids come from a process-global
  // counter that starts at zero again, so self-succession onto a re-minted id
  // would hand this pane a pty it never owned. Only a confirmed run change
  // retires them, since an events reconnect is not evidence of a restart and
  // may precede the handshake naming the ghost; an unproven answer is safe
  // because a ghost is acted on only while its stamped run is confirmed.
  useEffect(() => onServerRunChanged(() => heldConnIdsRef.current.clear()), [])
  // May a handshake naming `id` be treated as this pane meeting its own ghost?
  const ownGhostOfThisRun = (id: string): boolean => {
    const stamp = heldConnIdsRef.current.get(id)
    return stamp !== undefined && runIdentityConfirmedAs(stamp)
  }
  const connId = useMemo<ConnectionIdentity>(
    () => ({
      read: () => myConnIdRef.current,
      write: (next) => {
        if (next !== null) heldConnIdsRef.current.set(next, currentRunStamp())
        myConnIdRef.current = next
      },
    }),
    [],
  )
  // THE TAKE-OVER INTENT (see `channels.ts` for why it is state and not a
  // parked closure). The ref is the storage; the channel is the surface the
  // lifecycle and the coordinator see.
  const takeoverArmedRef = useRef(false)
  // The ghost a SELF-SUCCESSION expects to displace, sent as the resize frame's
  // `expected_owner`. Undefined for a PRESSED take-over, which may take from
  // anyone.
  const takeoverExpectedRef = useRef<string | undefined>(undefined)
  // A pressed take-over is in flight and unanswered. Distinct from the intent,
  // which is spent when the flagged frame goes out; this outlives it until an
  // ownership answer arrives, so a spine document predating the grant cannot
  // flash the card back over a pane the user just took. A press names no
  // expected owner and so cannot be refused, which is why blocking is safe.
  const pressedClaimRef = useRef(false)
  const takeoverIntent = useMemo<TakeoverIntent>(
    () => ({
      read: () => takeoverArmedRef.current,
      expectedOwner: () => takeoverExpectedRef.current,
      arm: (expectedOwner) => {
        takeoverArmedRef.current = true
        takeoverExpectedRef.current = expectedOwner
      },
      clear: () => {
        takeoverArmedRef.current = false
        takeoverExpectedRef.current = undefined
      },
    }),
    [],
  )

  // The other device's raw `User-Agent`. Two writers: the `pty.owner`
  // handover that demoted this client, and the connected handshake's seed
  // for a watcher that merely attached (gated on the events socket, which
  // is the only channel that can later correct the name).
  const [takeoverDevice, setTakeoverDevice] = useState<string | null>(null)
  // The connection id the name above was learned WITH. A kept name is only as
  // good as the owner it describes, so the two travel together and the spine's
  // `input_owner` is checked against this one rather than against nothing.
  const takeoverDeviceOwnerRef = useRef<string | null>(null)
  // The name as of now, not as of the render a socket callback closes over:
  // `seedFromConnected` runs from the PTY socket wired on the mount render, so
  // a state read there would see a permanently null prior name and downgrade a
  // good device title to the generic one.
  const takeoverDeviceRef = useRef<string | null>(null)
  // One writer for the trio, so a name can never be set without the id it names
  // or cleared without clearing it.
  const setTakeoverDeviceFor = (
    device: string | null,
    ownerId: string | null,
  ) => {
    takeoverDeviceOwnerRef.current = device === null ? null : ownerId
    takeoverDeviceRef.current = device
    setTakeoverDevice(device)
  }
  // Whether ANY connection drives this pty, as far as this client knows. It
  // starts true and pessimistic: before the handshake answers, "somebody might
  // be driving" is the copy that is never wrong, and a foregrounded pane that
  // turns out to own the pty never renders the card at all.
  const [ownerPresent, setOwnerPresent] = useState(true)
  // True once this PTY socket has EXHAUSTED its reconnect budget and emitted
  // `failed`. Distinct from "still retrying": this is a hard stop that surfaces
  // an explicit Reconnect affordance. Only meaningful when the app is NOT
  // globally offline, which the pane's own overlay precedence handles.
  const [connectionLost, setConnectionLost] = useState(false)
  // Whether the server has answered "who drives this pty" on this mount. The
  // initial verdict is a foreground guess and nothing more, so the input surface
  // waits for this before it summons a keyboard.
  const [handshakeSeen, setHandshakeSeen] = useState(false)

  // Device names survive events reconnects and are replaced only by a handover,
  // handshake, or a refetched spine owner. The same spine read corrects the
  // ownership verdict when a conditional self-succession was refused silently.
  // The demotion rule is:
  //
  //   - The spine must name somebody: `undefined` is the server declining to
  //     answer and `null` is nobody driving, neither evidence against us.
  //   - It must not name us.
  //   - It must not name one of our own ghosts: a self-succession about to be
  //     granted still has the pty recorded to this pane's dead connection.
  //   - No pressed claim of ours may be outstanding, since a press cannot be
  //     refused and only a stale document could disagree. A self-succession
  //     deliberately does not block it, because refusal is an answer it can get
  //     and the ghost clause already covers the version that will be granted.
  useEffect(() => {
    if (conn !== "open") return
    // The server has not answered the question: no evidence, so no correction.
    if (spineInputOwner === undefined) return
    const myConnId = connId.read()
    // The spine naming us is the grant landing, whichever way it reached us.
    if (spineInputOwner !== null && spineInputOwner === myConnId) {
      pressedClaimRef.current = false
    }
    const named = takeoverDeviceOwnerRef.current
    if (named !== null && spineInputOwner !== named) setTakeoverDeviceFor(null, null)
    if (!ownership.read()) return
    if (spineInputOwner === null) return
    if (spineInputOwner === myConnId) return
    if (heldConnIdsRef.current.has(spineInputOwner)) return
    if (pressedClaimRef.current) return
    ownership.write(false)
    setOwnerPresent(true)
  }, [conn, spineInputOwner, connId, ownership])

  // A `pty.owner` event confirms this connection, demotes it for another owner,
  // or marks the PTY unowned. Demotion is sticky until a fresh handshake or an
  // explicit take-over; visibility alone never claims ownership.
  useEffect(() => {
    return onPtyOwner((ptyId, ownerId, device) => {
      if (ptyId !== id) return
      // Any handover for this pty is the server ANSWERING, so a pressed claim of
      // ours is no longer in flight and stops shielding the verdict from the
      // spine. This event is the definitive word either way.
      pressedClaimRef.current = false
      const freed = ownerId === undefined || ownerId === null
      const mine = isOwnerAfterHandover(ownerId, myConnIdRef.current)
      setOwnerPresent(!freed)
      // Through the channel, not an inline copy of its body: the verdict has
      // ONE write implementation, so anything the channel ever grows reaches
      // this, the highest-traffic transition, by construction.
      ownership.write(mine)
      if (!mine) {
        // An event that does not name this connection retires the armed intent;
        // the bounce handshake handles an unowned PTY without carrying it forward.
        takeoverIntent.clear()
      }
      // Remember which device took over (for the placeholder's copy) while
      // demoted; clear it the moment ownership returns.
      setTakeoverDeviceFor(mine ? null : (device ?? null), mine ? null : (ownerId ?? null))
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  // Publish this pane's verdict into the store ledger so surfaces outside the
  // pane (the agent ⋯ menu) can disable mutating actions while another device
  // drives the agent. Agent PTYs only: a companion terminal taken over
  // elsewhere says nothing about the agent. The verdict is the ledger's fast
  // path in both directions, gating the menu when a handover lands and
  // un-gating it after a take-over, while the spine's `input_owner` still names
  // the previous owner until the refetch. A pane whose socket has failed for
  // good publishes no verdict, and the cleanup hands the answer back to the
  // server-published spine field alone.
  useEffect(() => {
    if (kind !== "agent") return
    if (connectionLost) {
      noteAgentPtyOwnership(id, "unknown")
      return
    }
    noteAgentPtyOwnership(id, isOwner ? "mine" : "elsewhere")
    return () => noteAgentPtyOwnership(id, "unknown")
  }, [kind, id, isOwner, connectionLost])

  // The connected handshake replaces the foreground guess unless a take-over is
  // armed. It can be stale, riding the PTY socket while `pty.owner` rides the
  // events socket with nothing ordering the two, so a strictly newer applied
  // `pty.owner` wins: verdict and `ownerPresent` both defer on the same
  // `handshakeSuperseded` comparison. Without that, a slow
  // `connected{owner:null}` after a fresh `pty.owner{owner:B}` would re-seed
  // this client as a phantom owner that no further event corrects.
  function seedFromConnected(
    myConnId: string,
    owner: HandshakeOwner,
    ownerEpoch?: number,
    ownerDevice?: string,
  ) {
    setHandshakeSeen(true)
    const appliedEpoch = appliedPtyOwnerEpoch(id)
    const superseded = handshakeSuperseded(ownerEpoch, appliedEpoch)
    if (!superseded) {
      setOwnerPresent(owner === undefined ? true : owner !== null)
    }
    // Self-succession, the blipped owner's half of "losing ownership is
    // sticky". The server reaps a dead connection by send failure, tens of
    // seconds later, while a blipped client is back in about one with a fresh
    // id, so its handshake names its own dead id as the driver and a plain
    // comparison would demote it to a watcher of its own ghost with nothing to
    // correct it.
    //
    // The id is therefore matched against every id this pane has held, since a
    // flapping radio can produce a handshake still naming the connection from
    // before the previous one. The run must also be confirmed, because a
    // restart mints ids from zero again: an unproven run means no succession
    // and one tap for the returning driver (see `ownGhostOfThisRun`).
    //
    // The claim goes out as a take-over naming the ghost it expects to
    // displace, so a frame delayed on a radio cannot steal a pty somebody
    // legitimately claimed in the gap; the client lands as a watcher instead.
    //
    // A backgrounded page never self-succeeds (a departed owner comes back as a
    // watcher and presses the button), and neither does a superseded handshake.
    if (
      !superseded &&
      typeof owner === "string" &&
      owner !== myConnId &&
      ownGhostOfThisRun(owner) &&
      isForeground()
    ) {
      takeoverIntent.arm(owner)
    }
    const mine = seedVerdictFromConnected({
      owner,
      myConnId,
      foreground: isForeground(),
      takeoverArmed: takeoverIntent.read(),
      handshakeEpoch: ownerEpoch,
      appliedEpoch,
      priorVerdict: ownership.read(),
    })
    ownership.write(mine)
    // Seed the other device's name from the same frame: a watcher that merely
    // attached hears no `pty.owner` broadcast, so this is its only chance at a
    // specific name. Gated on the events socket being open, because those
    // broadcasts are the only thing that can correct a name and one planted
    // while the socket is down would go stale with no correction coming. The
    // verdict seed above is deliberately not gated, since the generic title is
    // never wrong.
    if (conn === "open") {
      const next = seedDeviceFromConnected({
        mine,
        superseded,
        owner,
        ownerDevice,
        priorDevice: takeoverDeviceRef.current,
      })
      setTakeoverDeviceFor(next, typeof owner === "string" ? owner : null)
    }
  }

  /// The PTY socket own connection state, delivered by the lifecycle.
  ///
  /// `failed` is the hard stop that means LOST, cleared by any retry or reopen.
  /// Any `closed` retires an armed take-over, so an intent never outlives the
  /// socket it was armed for. The take-over's own deliberate close does not
  /// reach here, because `connect()` detaches the orphan handlers first, which
  /// is how the intent survives the one bounce it is meant to ride.
  function notePtyConn(state: ConnState) {
    if (state === "failed") {
      setConnectionLost(true)
      takeoverIntent.clear()
      // A press whose socket died was never answered and never will be on this
      // connection, so it stops shielding the verdict along with the intent.
      pressedClaimRef.current = false
      return
    }
    if (state === "closed") {
      takeoverIntent.clear()
      pressedClaimRef.current = false
      return
    }
    if (state === "connecting" || state === "open") setConnectionLost(false)
  }

  // Take-over is a fresh attach: bouncing the socket resets viewer-era buffer
  // state, and the armed intent rides the new connection's first resize frame.
  function takeOver() {
    // Idempotent while the bounce is in flight. A second press must not close
    // a socket that is still opening: the intent is already armed and the frame
    // that carries it has not gone out yet.
    if (takeoverIntent.read()) return
    takeoverIntent.arm()
    pressedClaimRef.current = true
    ownership.write(true)
    // Clear the other device's name as ownership is optimistically claimed,
    // honoring the invariant that the name only ever names a device we do NOT
    // own.
    setTakeoverDeviceFor(null, null)
    const pty = ptyRef.current
    if (pty) {
      // `onReconnecting` fires only when a drop schedules a retry, so nothing
      // else raises the cue for a deliberate `connect()` and the window would
      // read as a frozen terminal. `onOpen` clears it.
      setReconnecting(true)
      // One call, whatever state the socket is in: `connect()` detaches and
      // closes a live socket before reopening, and refills the retry budget of
      // one that gave up; dead sockets follow the same path.
      pty.connect()
    }
    // Refocus waits for confirmed ownership and the current attach replay, so a
    // phone does not raise its keyboard over a still-reconnecting pane.
  }

  return {
    isOwner,
    ownership,
    connId,
    takeoverIntent,
    seedFromConnected,
    notePtyConn,
    // Parsing lives in the pure, tested `deviceLabel` helper.
    takeoverLabel: deviceLabel(takeoverDevice),
    ownerPresent,
    handshakeSeen,
    connectionLost,
    setConnectionLost,
    takeOver,
  }
}
