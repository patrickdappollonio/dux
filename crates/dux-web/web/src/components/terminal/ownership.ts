// THE OWNERSHIP MACHINE.
//
// A PTY is shared across every connected device, but only ONE of them drives
// its size and may type into it; the others render a read-only take-over
// placeholder, so two people cannot fight over one prompt.
//
// FOUR STATES, and every one of them is somewhere in this file:
//
//   OWNER         this client drives the PTY. Typing surfaces render, input is
//                 forwarded, resizes go out.
//   OBSERVER      another device drives it. The take-over card is up, every
//                 write path returns early, and no resize is sent.
//   CLAIM-PENDING a take-over fired before this socket had a connection id (or
//                 over a socket that could not carry the frame). The claim is
//                 parked and the next `connected` frame performs it.
//   LOST          this socket spent its reconnect budget. The pane still knows
//                 what it believed, but it stops publishing that belief,
//                 because a stale "mine" from a dead connection would override
//                 the server's own field forever on a surface that cannot type.
//
// SIX TRANSITION SITES, and there are no others:
//
//   1. the INITIAL guess: a foregrounded pane claims on attach by sending its
//      size, a backgrounded one attaches as a silent observer.
//   2. a `pty.owner` HANDOVER: a definitive id comparison, never a timing or
//      echo heuristic. The old guess inverted when two devices claimed in the
//      same instant and the broadcast order flipped, leaving BOTH on the
//      placeholder. A missing id on either side reads as "not us".
//   3. TAKE-OVER: an optimistic local flip plus the claim, or the parked claim
//      and a socket reopen when the claim cannot go out.
//   4. the parked claim being CONSUMED by the next `connected` frame (in the
//      lifecycle, through this machine's `pendingClaim` flag).
//   5. the socket's CONN STATE: `failed` is the hard stop that means LOST; any
//      retry or reopen clears it.
//   6. the EVENTS SOCKET going away, which drops the other device's NAME (never
//      the verdict): `pty.owner` is delivered live-only with no replay, so
//      across an outage the name goes stale while the generic copy is never
//      wrong.
//
// The verdict is published through a CHANNEL rather than read off state,
// because an in-flight keystroke has to be gated by the new answer at once,
// before the re-render that shows it lands. Writing the channel flips the
// synchronous read and the rendered state together, so they cannot diverge.
import { useEffect, useMemo, useRef, useState } from "react"
import type { Terminal } from "@xterm/xterm"

import { deviceLabel } from "@/lib/deviceLabel"
import type { PtySocket } from "@/lib/ptySocket"
import { isForeground, isOwnerAfterHandover, onPtyOwner } from "@/lib/ptyOwnership"
import { noteAgentPtyOwnership } from "@/lib/store"
import type { ConnState } from "@/lib/types"

import type { ConnectionIdentity, OwnershipVerdict } from "./channels"

export type TerminalOwnershipDeps = {
  /// The pty id: the session id for an agent, the terminal id for a companion.
  id: string
  kind: "agent" | "terminal"
  /// The EVENTS socket's state, which decides whether the other device's name
  /// can still be trusted.
  conn: ConnState
  termRef: { current: Terminal | null }
  ptyRef: { current: PtySocket | null }
  focusTypingSurface: () => void
}

export type TerminalOwnership = {
  /// The rendered verdict.
  isOwner: boolean
  /// The verdict channel, for the lifecycle's stable closures.
  ownership: OwnershipVerdict
  /// This socket's connection id, owned by the lifecycle's attach wiring and
  /// read here for the handover comparison.
  connId: ConnectionIdentity
  /// The parked claim, set by `takeOver` and consumed by the next `connected`
  /// frame.
  pendingClaimRef: { current: boolean }
  /// A human label for the device that took over ("Chrome on macOS"), or null
  /// when the other device's `User-Agent` was absent, unrecognized, or stale.
  takeoverLabel: string | null
  /// Whether this socket has given up for good.
  connectionLost: boolean
  setConnectionLost: (value: boolean) => void
  takeOver: () => void
}

export function useTerminalOwnership(
  deps: TerminalOwnershipDeps,
): TerminalOwnership {
  const { id, kind, conn, termRef, ptyRef, focusTypingSurface } = deps

  // SITE 1: the initial guess. No-document contexts read as foreground, so a
  // claim is never silently suppressed.
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
  const connId = useMemo<ConnectionIdentity>(
    () => ({
      read: () => myConnIdRef.current,
      write: (next) => {
        myConnIdRef.current = next
      },
    }),
    [],
  )
  const pendingClaimRef = useRef(false)

  // The other device's raw `User-Agent`, captured from the handover that
  // demoted this client.
  const [takeoverDevice, setTakeoverDevice] = useState<string | null>(null)
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
  useEffect(() => {
    return onPtyOwner((ptyId, ownerId, device) => {
      if (ptyId !== id) return
      const mine = isOwnerAfterHandover(ownerId, myConnIdRef.current)
      // Through the channel, not an inline copy of its body: the verdict has
      // ONE write implementation, so anything the channel ever grows reaches
      // this, the highest-traffic transition, by construction.
      ownership.write(mine)
      // Remember which device took over (for the placeholder's copy) while
      // demoted; clear it the moment ownership returns.
      setTakeoverDevice(mine ? null : (device ?? null))
    })
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

  // SITE 3. Reclaim ownership from another device. Sending the current size IS
  // the claim server-side (most-recent claim wins), so the PTY snaps back to
  // this viewport and this client's input is forwarded again. The channel flips
  // the synchronous read first, so the resize passes the owner gate before the
  // re-render lands. The server's resulting `pty.owner` carries this
  // connection's id, so the handover handler recognises it as ours by id and
  // keeps this client the owner.
  function takeOver() {
    ownership.write(true)
    // Clear the other device's name as ownership is optimistically claimed,
    // honoring the invariant that the name only ever names a device we do NOT
    // own.
    setTakeoverDevice(null)
    const term = termRef.current
    const pty = ptyRef.current
    if (term && pty) {
      // Only claim now if the connection id is known AND the socket can
      // actually carry the frame: the server stamps the resulting `pty.owner`
      // with that id, and this client must be able to recognise it as its own
      // or the handover echo would immediately revoke the optimistic claim.
      // `sendResize` reports whether the frame went on the wire, and its answer
      // is the third health check, since `isOpen` can be true a moment before a
      // close lands.
      const claimed =
        myConnIdRef.current !== null &&
        pty.isOpen &&
        pty.sendResize(term.rows, term.cols)
      if (!claimed) {
        // The claim could not be made: the id is unknown, or the socket is
        // closed, mid-reconnect, or has spent its retry budget for good.
        // Deferring to the next `connected` frame alone is not enough, because
        // a socket that gave up produces no further frames on its own and
        // nothing else ever reopens it: the pane would sit under an optimistic
        // "I own this" with a black terminal until the user switched agents
        // (which remounts, and it was that remount, not the take-over, that
        // fixed it).
        //
        // So reopen it here. `connect()` refills the retry budget, and the
        // reopen replays the server's scrollback through the existing
        // reset-then-repaint path, which is the ONLY thing that repaints this
        // viewport: the child's SIGWINCH redraw is a no-op when the size it is
        // told matches the size it already has. The parked claim then fires
        // from the resulting `connected` frame, once there is an id the
        // handover echo can be matched against.
        pendingClaimRef.current = true
        pty.connect()
      }
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
    pendingClaimRef,
    // Parsing lives in the pure, tested `deviceLabel` helper.
    takeoverLabel: deviceLabel(takeoverDevice),
    connectionLost,
    setConnectionLost,
    takeOver,
  }
}
