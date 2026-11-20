// A shared PTY has one driver. Plain attaches never steal; take-over arms an
// intent and reconnects so the claim rides a fresh attach and replay.
//
//   OWNER     the server has said this page drives; writes are enabled.
//   OBSERVER  another connection (or none while backgrounded) owns the PTY.
//   PENDING   the server has not said yet; no writes and no card.
//   CLAIMING  a take-over reconnect is waiting to send its flagged first resize.
//   LOST      a terminal socket failure suppresses the stale local verdict.
//
// A foreign owner named by the handshake, a `pty.owner` event or the spine
// demotes at once, whatever the socket is doing. Losing ownership remains
// sticky until an explicit take-over or confirmed self-succession.
import { useEffect, useMemo, useRef, useState } from "react"

import { deviceLabel } from "@/lib/deviceLabel"
import type { PtySocket } from "@/lib/ptySocket"
import {
  appliedPtyOwnerEpoch,
  forgetHeldConnIds,
  handshakeSuperseded,
  heldConnIdStamp,
  isForeground,
  isOwnerAfterHandover,
  onPtyOwner,
  rememberHeldConnId,
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

/// Whether this page drives the pty: on the server's word ("this", "not"), or
/// not known yet ("pending").
type Verdict = "this" | "not" | "pending"

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
  /// Whether the server has said this page drives the pty. False while the
  /// verdict is still pending, which `ownershipKnown` tells apart.
  isOwner: boolean
  /// Whether the server has said who drives, on this mount or since a word
  /// only the next handshake can settle. While false the pane shows neither
  /// input nor the take-over card.
  ownershipKnown: boolean
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
  /// least once on this mount.
  handshakeSeen: boolean
  /// Whether the server has named this pane's own driver id as the row's
  /// owner on the current connection: the handshake or a `pty.owner` event.
  /// Until then an owner verdict over a row nobody had claimed is only leave
  /// to claim it, which a pane off screen must not act on.
  rowConfirmed: boolean
  readRowConfirmed: () => boolean
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

  // The channel's boolean is "this page drives, on the server's word", so every
  // write gate reads PENDING as no.
  const [verdict, setVerdictState] = useState<Verdict>("pending")
  const verdictRef = useRef<Verdict>("pending")
  const setVerdict = (next: Verdict) => {
    verdictRef.current = next
    setVerdictState(next)
  }
  const ownership = useMemo<OwnershipVerdict>(
    () => ({
      read: () => verdictRef.current === "this",
      write: (mine) => {
        verdictRef.current = mine ? "this" : "not"
        setVerdictState(verdictRef.current)
      },
    }),
    [],
  )
  const isOwner = verdict === "this"

  const myConnIdRef = useRef<string | null>(null)
  const [rowConfirmed, setRowConfirmedState] = useState(false)
  const rowConfirmedRef = useRef(false)
  const setRowConfirmed = (confirmed: boolean) => {
    rowConfirmedRef.current = confirmed
    setRowConfirmedState(confirmed)
  }
  // Every id this page has ever held (`rememberHeldConnId`), not merely this
  // pane's last: a returning owner recognises its own dead connection in the
  // next handshake (see the self-succession rule in `seedFromConnected`), a
  // flapping radio can produce two handshakes where the second still names the
  // connection from before the first, and a device drives a whole row, so the
  // ghost can be the connection this page held on another tab or terminal of
  // the same agent. Each id is stamped with the server run it was learned
  // under, because a restarted server mints ids from zero again and another
  // device's fresh id can equal one of ours (see `serverRun.ts`).
  //
  // Ghosts do not survive a server restart: ids come from a process-global
  // counter that starts at zero again, so self-succession onto a re-minted id
  // would hand this pane a pty it never owned. Only a confirmed run change
  // retires them, since an events reconnect is not evidence of a restart and
  // may precede the handshake naming the ghost; an unproven answer is safe
  // because a ghost is acted on only while its stamped run is confirmed.
  useEffect(() => onServerRunChanged(forgetHeldConnIds), [])
  // May a handshake naming `id` be treated as this page meeting its own ghost?
  const ownGhostOfThisRun = (id: string): boolean => {
    const stamp = heldConnIdStamp(id)
    return stamp !== undefined && runIdentityConfirmedAs(stamp)
  }
  const connId = useMemo<ConnectionIdentity>(
    () => ({
      read: () => myConnIdRef.current,
      write: (next) => {
        if (next !== null) rememberHeldConnId(next, currentRunStamp())
        // A new connection has not been named the row's driver yet.
        if (next !== myConnIdRef.current) {
          rowConfirmedRef.current = false
          setRowConfirmedState(false)
        }
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
  // Whether the newest `pty.owner` this mount heard said nobody drives the pty.
  // A handshake superseded by that word is about a row that was let go of
  // between the snapshot and the event.
  const newestEventFreedRef = useRef(false)
  // The owner the newest applied `pty.owner` named, or null when it freed the
  // row. A pane whose handshake is still in flight has no id to compare an
  // event with, so the comparison is made again when the handshake lands.
  const newestEventOwnerRef = useRef<string | null>(null)
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
  // Whether the handshake has answered "who drives this pty" on this mount. The
  // ref is for the spine correction, which must not let a document the page
  // loaded before this mount's first handshake decide (see below).
  const [handshakeSeen, setHandshakeSeen] = useState(false)
  const handshakeSeenRef = useRef(false)
  // Whether this mount has heard any `pty.owner` for its pty. When it has, the
  // newest one it heard is the newest one applied, which a superseded handshake
  // defers to.
  const eventHeardRef = useRef(false)

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
  //   - Before this mount's first handshake it decides nothing: after a reload
  //     the spine can still name the replaced page, which this one cannot recognise.
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
    if (verdictRef.current === "not") return
    if (spineInputOwner === null) return
    if (spineInputOwner === myConnId) return
    if (heldConnIdStamp(spineInputOwner) !== undefined) return
    if (pressedClaimRef.current) return
    if (!handshakeSeenRef.current) return
    ownership.write(false)
    setOwnerPresent(true)
  }, [conn, spineInputOwner, connId, ownership])

  // Handles a `pty.owner` heard while this socket has no id that names one of
  // this page's ids or frees the row, which only the next handshake can judge;
  // returns false for a foreign owner, which demotes as usual.
  const awaitsHandshake = (ownerId: string | undefined, freed: boolean): boolean => {
    takeoverIntent.clear()
    if (freed) {
      if (verdictRef.current === "not") return false
      setVerdict("pending")
      setOwnerPresent(false)
      setTakeoverDeviceFor(null, null)
      return true
    }
    const stamp = ownerId === undefined ? undefined : heldConnIdStamp(ownerId)
    if (stamp === undefined) return false
    setOwnerPresent(true)
    setTakeoverDeviceFor(null, null)
    if (verdictRef.current === "this" && runIdentityConfirmedAs(stamp)) return true
    setVerdict("pending")
    return true
  }

  // A `pty.owner` event confirms this connection, demotes it for another owner,
  // or marks the PTY unowned. Demotion is sticky until a fresh handshake or an
  // explicit take-over; visibility alone never claims ownership.
  useEffect(() => {
    return onPtyOwner((ptyId, ownerId, device) => {
      if (ptyId !== id) return
      eventHeardRef.current = true
      // Any handover for this pty is the server ANSWERING, so a pressed claim of
      // ours is no longer in flight and stops shielding the verdict from the
      // spine. This event is the definitive word either way.
      pressedClaimRef.current = false
      const freed = ownerId === undefined || ownerId === null
      newestEventFreedRef.current = freed
      newestEventOwnerRef.current = freed ? null : (ownerId ?? null)
      // A self-succession still waiting to be sent outlives its ghost letting
      // go: the row is free, and the server claims a free row for that frame
      // as it would for a plain resize, displacing nobody. Dropping the intent
      // here left a page that switched from an agent's tab to its terminal
      // watching a row it had just been driving.
      if (freed && takeoverIntent.read() && takeoverIntent.expectedOwner() !== undefined) {
        setOwnerPresent(false)
        return
      }
      if (myConnIdRef.current === null && awaitsHandshake(ownerId, freed)) return
      const mine = isOwnerAfterHandover(ownerId, myConnIdRef.current)
      setRowConfirmed(mine)
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
  // drives the agent. Agent PTYs only, because the ledger is keyed by tab; a
  // device drives a whole row, so an agent tab's verdict already speaks for
  // the agent's terminals too. The verdict is the ledger's fast
  // path in both directions, gating the menu when a handover lands and
  // un-gating it after a take-over, while the spine's `input_owner` still names
  // the previous owner until the refetch. A pane whose socket has failed for
  // good, or whose verdict is still pending, publishes no verdict, and the
  // cleanup hands the answer back to the server-published spine field alone.
  useEffect(() => {
    if (kind !== "agent") return
    if (connectionLost || verdict === "pending") {
      noteAgentPtyOwnership(id, "unknown")
      return
    }
    noteAgentPtyOwnership(id, verdict === "this" ? "mine" : "elsewhere")
    return () => noteAgentPtyOwnership(id, "unknown")
  }, [kind, id, verdict, connectionLost])

  // The connected handshake settles the verdict unless a take-over is armed.
  // It can be stale, riding the PTY socket while `pty.owner` rides the
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
    handshakeSeenRef.current = true
    const appliedEpoch = appliedPtyOwnerEpoch(id)
    const superseded = handshakeSuperseded(ownerEpoch, appliedEpoch)
    if (!superseded) {
      setOwnerPresent(owner === undefined ? true : owner !== null)
      // An old server that does not answer grants any claim, as it always has.
      setRowConfirmed(owner === undefined || owner === myConnId)
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
    // watcher and presses the button). A superseded handshake judges the newer
    // event this mount heard instead of its own stale owner.
    const newestOwner: HandshakeOwner = !superseded
      ? owner
      : eventHeardRef.current
        ? newestEventOwnerRef.current
        : undefined
    if (
      typeof newestOwner === "string" &&
      newestOwner !== myConnId &&
      ownGhostOfThisRun(newestOwner) &&
      isForeground()
    ) {
      takeoverIntent.arm(newestOwner)
    }
    // This page's own ghost let go of the row after the snapshot: a foregrounded
    // page claims it with a plain resize. A row another device let go of stays
    // that device's until a press, since losing ownership is sticky.
    const ghostLetGo =
      superseded &&
      eventHeardRef.current &&
      newestEventFreedRef.current &&
      typeof owner === "string" &&
      owner !== myConnId &&
      ownGhostOfThisRun(owner) &&
      isForeground()
    // The page's other pane claimed the row on the driver id this socket was
    // just seated on, before this handshake landed.
    const eventNamedThisSeat =
      superseded && newestEventOwnerRef.current === myConnId
    if (eventNamedThisSeat) setRowConfirmed(true)
    // A superseded handshake keeps the verdict the newer event left, and a
    // pending one is not a verdict that this page drives.
    const mine =
      eventNamedThisSeat ||
      ghostLetGo ||
      seedVerdictFromConnected({
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
    ownershipKnown: verdict !== "pending",
    handshakeSeen,
    rowConfirmed,
    readRowConfirmed: () => rowConfirmedRef.current,
    connectionLost,
    setConnectionLost,
    takeOver,
  }
}
