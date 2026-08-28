// THE OWNERSHIP MACHINE.
//
// A PTY is shared across every connected device, but only ONE of them drives
// its size and may type into it; the others render a read-only take-over
// placeholder, so two people cannot fight over one prompt.
//
// TWO RULES SIT UNDER EVERYTHING HERE, and they are the whole shape of this
// file:
//
//   ATTACHING NEVER STEALS. A plain resize claims only an UNOWNED pty
//   server-side. Opening a phone onto an agent your desktop is driving makes you
//   a watcher, and staying a watcher is the correct answer until you say
//   otherwise. Ownership therefore does NOT follow focus: a desktop that was
//   taken over stays a watcher when refocused, and one tap gets it back. That is
//   the deliberate cost of killing the silent steal and the SIGWINCH ping-pong
//   that came with it.
//
//   TAKE-OVER IS A FRESH ATTACH. The button does not write a claim down the live
//   socket; it arms an intent and BOUNCES the socket. The reconnect drives the
//   existing replay machinery (fresh `connected` frame and generation, buffer
//   reset, server repaint, mode restore), so taking over structurally cannot
//   inherit the polluted viewer-era scrollback that a wide-owner/narrow-viewer
//   pair writes into this client's buffer. Ownership lags the reconnect by one
//   replay parse, because the claim rides the first resize frame of the new
//   connection.
//
// FOUR STATES, and every one of them is somewhere in this file:
//
//   OWNER         this client drives the PTY. Typing surfaces render, input is
//                 forwarded, resizes go out.
//   OBSERVER      another device drives it, OR nobody does and this pane is
//                 backgrounded. The take-over card is up (with copy that says
//                 which), every write path returns early, and no resize is sent.
//   CLAIMING      a take-over is armed and the socket is bouncing. The verdict
//                 already reads "mine" so the first resize of the new connection
//                 passes the owner gate and carries the flag.
//   LOST          this socket spent its reconnect budget. The pane still knows
//                 what it believed, but it stops publishing that belief,
//                 because a stale "mine" from a dead connection would override
//                 the server's own field forever on a surface that cannot type.
//
// SEVEN TRANSITION SITES, and there are no others:
//
//   1. the INITIAL guess: foreground, held only until the handshake answers.
//   2. a `pty.owner` HANDOVER: a definitive id comparison, never a timing or
//      echo heuristic, which inverts when two devices claim in the same
//      instant and the broadcast order flips, leaving BOTH on the
//      placeholder. A missing id on either side reads as "not us".
//   3. TAKE-OVER: arm the intent, flip the verdict optimistically, bounce the
//      socket. Idempotent while the bounce is in flight.
//   4. the `connected` HANDSHAKE re-seeding the verdict from the server's own
//      answer (`seedVerdictFromConnected`, called by the lifecycle). This is
//      what stops a foregrounded arrival wedging itself as a phantom owner now
//      that a refused claim emits nothing. It is also where SELF-SUCCESSION
//      lives: a handshake naming this pane's own previous, dead connection id
//      is a blipped owner meeting its own ghost, and a foregrounded page takes
//      its pty back with a flagged claim.
//   5. an OWNER-CLEARED `pty.owner` (the driver disconnected): every client
//      demotes and the card re-titles itself to "Running in the background".
//      NOBODY
//      CLAIMS. Losing ownership is sticky until a deliberate act, and sitting
//      on an open card is not one.
//   6. the socket's CONN STATE: `failed` is the hard stop that means LOST; any
//      retry or reopen clears it.
//   7. the EVENTS SOCKET going away, which drops the other device's NAME (never
//      the verdict): `pty.owner` is delivered live-only with no replay, so
//      across an outage the name goes stale while the generic copy is never
//      wrong.
//
// The verdict is published through a CHANNEL rather than read off state,
// because an in-flight keystroke has to be gated by the new answer at once,
// before the re-render that shows it lands. Writing the channel flips the
// synchronous read and the rendered state together, so they cannot diverge.
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
  /// Who the SPINE says drives this pty, refetched on every events-socket open.
  /// `null` when the server says nobody, `undefined` when it has not answered
  /// (an older server, or a view that carries no such field). It is the only
  /// thing that can correct a device NAME kept across an outage; see the name's
  /// own comment below.
  spineInputOwner?: string | null
  /// The pane's reconnect cue. The take-over bounce closes the socket
  /// deliberately, which fires no `onReconnecting` of its own (see
  /// `ReconnectingSocket.connect`), so the cue is raised here or the half-second
  /// window reads as a dead terminal rather than a reconnecting one.
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
  /// Re-seed the verdict from the `connected` handshake. Called by the
  /// lifecycle, which is where the frame lands. `ownerEpoch` is the handshake's
  /// `owner_epoch` stamp (undefined on an old server), which lets the seed
  /// defer to a strictly newer `pty.owner` already applied for this pty.
  /// `ownerDevice` is the handshake's `owner_device` (the owner's captured
  /// User-Agent; undefined on an old server or when there is none to name),
  /// which seeds the take-over card's device name for a watcher that merely
  /// attached and will therefore hear no `pty.owner` broadcast.
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

  // SITE 1: the initial guess, and now ONLY a guess: it holds for the handful of
  // milliseconds before the `connected` handshake answers (site 4), and after
  // that the server's answer decides. No-document contexts read as foreground,
  // so a claim is never silently suppressed.
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
  // THE GHOSTS: EVERY id this pane has ever held, not merely the last one.
  //
  // A returning owner has to recognise its own dead connection in the next
  // handshake's answer (see the self-succession rule in `seedFromConnected`),
  // and a single previous id is not enough to do that reliably: a flapping radio
  // can produce two handshakes in a row, and the second one may still name the
  // connection from before the first. Matching against the whole set means a
  // dropped intent between two handshakes cannot land the returning driver as a
  // watcher of itself.
  //
  // Pane-local and re-derived per handshake, so it is bounded by this pane's own
  // reconnect count and dies with the mount.
  //
  // Each id is STAMPED with the server run it was learned under, because a
  // restarted server mints ids from zero again and another device's fresh id can
  // equal one of ours (see `serverRun.ts`).
  const heldConnIdsRef = useRef<Map<string, number>>(new Map())
  // GHOSTS DO NOT SURVIVE A SERVER RESTART. Connection ids come from a
  // process-global counter that starts again at zero, so an id this pane held
  // against the previous run can be minted afresh for somebody else's
  // connection, and self-succession would then hand this pane a pty it never
  // owned.
  //
  // ONLY A CONFIRMED CHANGE RETIRES THEM, and the previous signal (the epoch
  // reset, which the store fires on every events reconnect) was the bug that
  // killed self-succession in the exact case it exists for: an ordinary mobile
  // drop takes BOTH sockets, so the events socket reconnects and clears this set
  // strictly before the pty handshake that names the ghost arrives, and the
  // returning driver lands on the take-over card. A reconnect is not a restart.
  //
  // The stamp on each id is the other half, and it is what makes an UNPROVEN
  // answer safe: a ghost is only ever ACTED ON while the current run is
  // confirmed to be the one it was learned under, so a probe that cannot answer
  // costs the returning driver one tap rather than letting it succeed onto an id
  // a restarted server may have handed to somebody else.
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
  // A PRESSED take-over of ours is in flight and the server has not answered.
  // Distinct from the intent, which is spent the moment the flagged frame goes
  // out; this outlives it, until an ownership answer arrives. It exists for one
  // job: keeping a spine document that predates the grant from flashing the card
  // back over a pane the user has just taken. A press cannot be refused (it names
  // no expected owner, so the server grants it unconditionally), so blocking on
  // it costs nothing in correctness.
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
  // The name as of NOW, rather than as of the render whose closure a socket
  // callback happens to be holding. `seedFromConnected` runs from the PTY
  // socket, which was wired on the mount render, so reading the state variable
  // there pins it to that render forever: the superseded branch then saw a
  // permanently null prior name and downgraded a perfectly good "Open on Chrome
  // on macOS" to the generic title. Refs are how every other read in this file
  // crosses that boundary.
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

  // SITE 6, REVERSED. Losing the events socket used to WIPE the device name
  // while `ownerPresent` stayed true, so a flapping spine downgraded a perfectly
  // good "Open on Chrome on Linux" to "Active on another device" and back again.
  // The wipe was defending against a name going stale with no correction coming;
  // the correction now exists, so the name is KEPT and only ever REPLACED by a
  // newer fact:
  //
  //   - a `pty.owner` handover (a definitive new owner and device), or
  //   - a `connected` handshake's owner snapshot, or
  //   - the check below, which compares the id the name was learned with against
  //     the SPINE's `input_owner` once the events socket is back and the spine
  //     has been refetched. A mismatch (or the spine saying nobody drives) means
  //     the name describes a device that is no longer driving, and it goes.
  //
  // The spine carries `input_owner` for companion terminals as well as agent
  // tabs, so a terminal's card gets the same correction an agent's does.
  //
  // AND THE SAME READ CORRECTS THE VERDICT, not merely the name. That half was
  // missing and it left a real hole. `seedVerdictFromConnected` returns true the
  // instant a self-succession arms, which used to be right by construction
  // because a flagged claim was granted unconditionally; the server now REFUSES
  // it, silently, when the ghost no longer holds the pty. The pane was then a
  // phantom owner: typing surfaces up, no card, every keystroke dying at the
  // server write gate, and no broadcast coming to say so, because a refusal
  // changes nothing and emits nothing.
  //
  // THE RULE, and each clause is load-bearing:
  //
  //   - The spine must NAME somebody. `undefined` is the server declining to
  //     answer and `null` is nobody driving; neither is evidence against us.
  //   - It must not name US.
  //   - It must not name one of OUR OWN GHOSTS. A self-succession about to be
  //     GRANTED is exactly the case where the pty is still recorded to this
  //     pane's dead connection, so demoting on it would flash the card over the
  //     returning driver a moment before the grant lands.
  //   - No PRESSED claim of ours may be outstanding. A press names no expected
  //     owner, so the server grants it unconditionally and the optimism is always
  //     right; only a spine document rendered before the grant could disagree,
  //     and that is staleness rather than refusal. A SELF-SUCCESSION deliberately
  //     does NOT block the demotion, because refusal is exactly the answer it can
  //     get, and the ghost clause above already protects the version of it that
  //     will be granted.
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

  // SITE 2. The server broadcasts a `pty.owner` carrying the claimer's
  // connection id; the store fans it out by pty id plus that owner id. For OUR
  // pty the owner id is compared against this socket's own connection id: an
  // equal id confirms our own claim (stay the owner), a different id means
  // another device took over (demote to the placeholder). Keyed by `id` so a
  // focus switch re-subscribes for the new target.
  // SITE 5 lives here too: an event with NO owner is the server saying the
  // driver disconnected and nobody holds the pty. Every client reads that as
  // "not me" (a missing id is "not us" by rule), so the fan-out below demotes
  // everyone, and that is ALL it does. LOSING OWNERSHIP IS STICKY: the
  // broadcast re-titles the card to "Running in the background" and claims
  // nothing,
  // whatever this pane's visibility is.
  //
  // There used to be a passive claim here, taken by any mounted foregrounded
  // viewer. It was the thing that beat a blipped owner back to its own pty: the
  // server's liveness reap is send-failure based and lands tens of seconds
  // after the drop, by which time the real owner has reconnected, so an idle
  // desktop sitting on an open card won a race the returning driver did not
  // know it was in. The four legitimate re-claim gestures all funnel through a
  // fresh handshake or the card's own button instead, and the blipped owner's
  // half of that is the self-succession rule in `seedFromConnected`.
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
        // ANY event that does not name us retires an armed take-over WITHOUT
        // sending it: re-arming is the user's decision, not a retry loop's.
        //
        // The FREED exemption that used to live here is gone. It parked the
        // intent through the old owner's disconnect so a mid-bounce take-over
        // could still claim flagged, but the bounce's own handshake finds the
        // pty UNOWNED and seeds a plain claim that reaches exactly the same
        // outcome. Keeping the flag alive past its socket was the cost, and
        // the flag outliving its socket is the whole class of bug this rule
        // exists to close.
        takeoverIntent.clear()
      }
      // Remember which device took over (for the placeholder's copy) while
      // demoted; clear it the moment ownership returns.
      setTakeoverDeviceFor(mine ? null : (device ?? null), mine ? null : (ownerId ?? null))
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  // Publish this pane's verdict into the store ledger so surfaces OUTSIDE the
  // pane (the agent ⋯ menu) can disable mutating actions while another device
  // drives the agent. Agent PTYs only: a companion terminal taken over
  // elsewhere says nothing about the agent itself. The verdict is the ledger's
  // fast path in BOTH directions: "elsewhere" gates the menu the moment the
  // handover frame lands, and "mine" un-gates it right after a take-over, while
  // the spine's `input_owner` still names the previous owner until the refetch.
  // "mine" starts as the same optimistic foreground guess `isOwner` itself
  // starts from; it is corrected by the handovers. A pane whose socket has
  // FAILED for good publishes NO verdict at all (the LOST state). The cleanup
  // retires the verdict; it also runs between re-publishes (any dep flip),
  // which is harmless because the new verdict lands in the same synchronous
  // pass, and on unmount it is what hands the answer back to the
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

  // SITE 4. The `connected` handshake, delivered by the lifecycle. The server
  // says who is driving; that answer replaces the foreground guess (except while
  // a take-over is armed, which `seedVerdictFromConnected` handles). This is
  // where a phone opening a desktop-driven agent learns it is a watcher, and it
  // is the reason a silently-refused claim can no longer wedge a pane as a
  // phantom owner.
  //
  // The handshake can also be STALE: it rides the PTY socket while `pty.owner`
  // rides the events socket, and nothing orders the two connections. When a
  // `pty.owner` with a strictly newer epoch has already been applied for this
  // pty, the seed keeps the verdict that event wrote (rule 2 in the pure
  // helper), and `ownerPresent` is likewise left as the newer event set it,
  // gated on the SAME `handshakeSuperseded` comparison so the two cannot
  // drift. Without the deferral, a slow `connected{owner:null}` landing after
  // a fresh `pty.owner{owner:B}` would re-seed this client as a phantom owner
  // that nothing ever corrects, because the stale-null direction emits no
  // further event.
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
    // SELF-SUCCESSION, the blipped owner's half of "losing ownership is
    // sticky". The server reaps a dead connection by send failure, which takes
    // tens of seconds; a client whose wifi blipped is back in about one, with a
    // freshly allocated connection id. Its handshake therefore names its OWN
    // previous, dead id as the driver, and a plain id comparison would demote
    // the returning owner to a watcher of its own ghost. Nothing would correct
    // it either: by the time the reap runs, `release` finds a different owner
    // recorded (or none) and broadcasts nothing at all.
    //
    // So the id is compared against EVERY id this pane has held, not merely the
    // most recent one, and a match on a FOREGROUNDED page is treated as
    // succeeding ourselves. The set matters: a flapping radio can produce two
    // handshakes in a row, and the second may still name the connection from
    // before the first, so a single-slot ghost would land the returning driver
    // as a watcher of itself.
    //
    // AND THE RUN MUST BE CONFIRMED. A ghost is only ours while the server is
    // still the run that minted it: a restart mints ids from zero again, so an
    // unproven run identity means no succession at all and the returning driver
    // pays one tap. See `ownGhostOfThisRun` and `serverRun.ts`.
    //
    // The claim goes out as a take-over, and it NAMES THE GHOST it expects to
    // displace. The server refuses the transfer inside its own critical section
    // when anybody else holds the pty by then, so a frame delayed on a mobile
    // radio cannot steal a pty somebody legitimately claimed in the gap; the
    // client then lands as a watcher with the card, exactly like a refused plain
    // resize. That expectation is what makes self-succession safe enough to be
    // the one press-less re-claim.
    //
    // A BACKGROUNDED page does not self-succeed: that is the C15/C16
    // backgrounded-owner contract, which says a departed owner comes back as a
    // watcher and presses the button. A superseded handshake does not either,
    // for the same reason rule 2 of the seed exists: another device's newer
    // claim has already been applied and this frame is stale.
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
    // Seed the other device's NAME from the same frame, through the pure rule
    // beside the verdict seed. A watcher that merely attached hears no
    // `pty.owner` broadcast at all, so this is its only chance at a specific
    // name; an owning pane never names another device, and a superseded
    // handshake keeps whatever the newer applied event wrote (functional
    // update, so the prior name is read at apply time rather than captured).
    // Gated on the EVENTS socket being open: `pty.owner` broadcasts are the
    // only thing that can ever correct a name, so a name planted while that
    // socket is down could go stale with no correction coming. The verdict
    // seed above is deliberately not gated; the generic title is never wrong.
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
  /// Two things hang off it. `failed` is the hard stop that means LOST, and any
  /// retry or reopen clears it. And ANY `closed` retires an armed take-over: the
  /// intent never outlives the socket it was armed for, so a press whose bounce
  /// failed is spent rather than parked, and the button works again. The
  /// take-over own deliberate close does not reach here, because `connect()`
  /// detaches the orphan handlers before closing it, which is precisely how the
  /// intent survives the one bounce it is meant to ride.
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

  // SITE 3. TAKE-OVER IS A FRESH ATTACH: arm the intent, flip the verdict, bounce
  // the socket.
  //
  // Nothing is written down the live socket any more, and that is the fix rather
  // than a simplification. A claim over the live socket left this client's buffer
  // exactly as it was, and a viewer's buffer is precisely the thing that is
  // polluted: while the owner drives a wider grid, every cursor-positioned
  // repaint overflows this narrower viewport and scrolls mangled wrapped rows
  // into the LOCAL scrollback. Resizing the PTY makes the child repaint cleanly
  // and clears nothing already recorded, so scrolling up after a take-over read
  // back garbage. Reconnecting instead routes through the machinery that already
  // exists for every reconnect: reset, server repaint (which clears the client's
  // scrollback), mode restore. Taking over IS a fresh attach, so it cannot
  // inherit viewer-era history.
  //
  // The claim itself rides the first resize frame of the NEW connection, flagged
  // by the armed intent; ownership therefore lags the press by one reconnect and
  // one replay parse. The stated cost: every take-over is now a
  // reset + replay + SIGWINCH rather than a single Text frame.
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
      // The socket is deliberately going down for about half a second. Nothing
      // else raises the cue for a deliberate `connect()` (`onReconnecting` fires
      // only when a DROP schedules a retry), so raise it here or the window
      // reads as a frozen terminal. `onOpen` clears it, as it does for any other
      // reconnect.
      setReconnecting(true)
      // One call, whatever state the socket is in: `connect()` detaches and
      // closes a live socket before reopening, and refills the retry budget of
      // one that gave up. The old dead-socket special case collapses into this.
      pty.connect()
    }
    // Deliberately NO refocus here. It used to call `focusTypingSurface()` at
    // once, which on a phone raises the soft keyboard over a pane that is a whole
    // reconnect and one replay parse away from having anything to type into. The
    // pane's own focus effect does it instead, when both facts are in: the
    // handshake has confirmed ownership and the replay for this attach epoch is
    // on screen.
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
