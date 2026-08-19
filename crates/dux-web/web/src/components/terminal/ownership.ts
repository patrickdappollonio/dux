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
//      echo heuristic. The old guess inverted when two devices claimed in the
//      same instant and the broadcast order flipped, leaving BOTH on the
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
//      demotes and the card re-titles itself to "Nobody is driving". NOBODY
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
  seedVerdictFromConnected,
  type HandshakeOwner,
} from "@/lib/ptyOwnership"
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
  focusTypingSurface: () => void
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
  seedFromConnected: (
    myConnId: string,
    owner: HandshakeOwner,
    ownerEpoch?: number,
  ) => void
  /// A human label for the device that took over ("Chrome on macOS"), or null
  /// when the other device's `User-Agent` was absent, unrecognized, or stale.
  takeoverLabel: string | null
  /// Whether SOMEBODY drives this pty right now, as far as this client knows.
  /// False means the driver disconnected and nobody has claimed it since, which
  /// the card says out loud rather than claiming a device is active. Only
  /// meaningful while `isOwner` is false.
  ownerPresent: boolean
  /// Whether this socket has given up for good.
  connectionLost: boolean
  setConnectionLost: (value: boolean) => void
  takeOver: () => void
}

export function useTerminalOwnership(
  deps: TerminalOwnershipDeps,
): TerminalOwnership {
  const { id, kind, conn, ptyRef, focusTypingSurface, setReconnecting } = deps

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
  // THE GHOST: the last id this pane held before the socket retired it. The
  // lifecycle nulls the live id on every drop, reopen and unmount, and this is
  // what the null takes the place of, because a returning owner has to be able
  // to recognise its own dead connection in the next handshake's answer (see
  // the self-succession rule in `seedFromConnected`).
  const prevConnIdRef = useRef<string | null>(null)
  const connId = useMemo<ConnectionIdentity>(
    () => ({
      read: () => myConnIdRef.current,
      write: (next) => {
        if (myConnIdRef.current !== null) {
          prevConnIdRef.current = myConnIdRef.current
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
  const takeoverIntent = useMemo<TakeoverIntent>(
    () => ({
      read: () => takeoverArmedRef.current,
      arm: () => {
        takeoverArmedRef.current = true
      },
      clear: () => {
        takeoverArmedRef.current = false
      },
    }),
    [],
  )

  // The other device's raw `User-Agent`, captured from the handover that
  // demoted this client.
  const [takeoverDevice, setTakeoverDevice] = useState<string | null>(null)
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

  // SITE 6. Drop the specific device name whenever the events socket is not
  // open. A handover is delivered live over `/ws/events` with NO replay on
  // reconnect, so if ownership changes while that socket is down this client
  // would otherwise keep naming a now-wrong device. The generic "Active on
  // another device" copy is never wrong, so it falls back to that across any
  // outage; a real handover after reconnect repopulates the name. Cleared on
  // the render-phase transition (React's "adjust state when input changes"
  // pattern) rather than in an effect, which avoids the extra
  // commit-then-clear render pass.
  const [prevConn, setPrevConn] = useState(conn)
  if (conn !== prevConn) {
    setPrevConn(conn)
    if (conn !== "open") setTakeoverDevice(null)
  }

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
  // broadcast re-titles the card to "Nobody is driving" and claims nothing,
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
      const freed = ownerId === undefined || ownerId === null
      const mine = isOwnerAfterHandover(ownerId, myConnIdRef.current)
      setOwnerPresent(!freed)
      // Through the channel, not an inline copy of its body: the verdict has
      // ONE write implementation, so anything the channel ever grows reaches
      // this, the highest-traffic transition, by construction.
      ownership.write(mine)
      if (!mine) {
        // A demotion retires any armed take-over WITHOUT sending it: this
        // client raced somebody else's claim and lost, and re-arming is the
        // user's decision, not a retry loop's.
        takeoverIntent.clear()
      }
      // Remember which device took over (for the placeholder's copy) while
      // demoted; clear it the moment ownership returns.
      setTakeoverDevice(mine ? null : (device ?? null))
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
  ) {
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
    // So the id is compared against the ghost as well as against the live id,
    // and a match on a FOREGROUNDED page is treated as succeeding ourselves.
    // The claim goes out as a take-over: the server grants a flagged claim
    // against any owner, and the owner being displaced here is a connection
    // this pane already knows is gone, so nothing is stolen from anyone. Arming
    // the intent is the whole mechanism, because the flag rides the first
    // resize frame of this new connection like any other take-over.
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
      owner === prevConnIdRef.current &&
      isForeground()
    ) {
      takeoverIntent.arm()
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
    // Whatever decided it, an owning pane never names another device: the name
    // only ever names a device this client does NOT own.
    if (mine) setTakeoverDevice(null)
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
    ownership.write(true)
    // Clear the other device's name as ownership is optimistically claimed,
    // honoring the invariant that the name only ever names a device we do NOT
    // own.
    setTakeoverDevice(null)
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
    // Refocus the active typing surface (the compose textarea when the mobile
    // compose bar is up, xterm's hidden textarea otherwise) so typing resumes
    // where it belongs the moment ownership returns.
    focusTypingSurface()
  }

  return {
    isOwner,
    ownership,
    connId,
    takeoverIntent,
    seedFromConnected,
    // Parsing lives in the pure, tested `deviceLabel` helper.
    takeoverLabel: deviceLabel(takeoverDevice),
    ownerPresent,
    connectionLost,
    setConnectionLost,
    takeOver,
  }
}
