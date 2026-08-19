// THE VIEWER-GRID MACHINE: what a non-owner does about a PTY that is sized for
// somebody else.
//
// ONE PTY HAS ONE AUTHORITATIVE GRID, the owner's. Every other attached browser
// renders the same byte stream into its own, differently sized xterm, so while
// a viewer watches, its live view is wrapped and clamped, and every repaint the
// child makes scrolls mangled rows into that viewer's LOCAL scrollback, where
// they stay until a fresh attach clears the buffer and rebuilds it from the
// server's repaint. Until the size arrived on the wire (the `connected`
// handshake's grid, and the `size` event after it) the viewer could not even
// know this was happening.
//
// TWO ANSWERS, and this machine owns both:
//
//   SAY SO. A viewer whose grid differs from the PTY's says it is sized for
//   another device. It is a statement of fact, not a control: the pane renders
//   it as a quiet, click-through badge.
//
//   HEAL BY RE-ATTACHING, NEVER BY RESIZING. A viewer must not resize the PTY,
//   because that is the silent steal this whole arc exists to kill. What it may
//   do is bounce its own socket, which runs the reconnect path that already
//   exists (reset, fresh generation, server repaint, mode restore) and so
//   replaces the polluted local buffer with a clean rebuild at the new
//   geometry. Healing is entirely the existing path; this machine only decides
//   WHEN to take it.
//
// FIVE GUARDS ON THE BOUNCE, all mandatory, and each one is a way the bounce
// would otherwise be wrong rather than merely wasteful:
//
//   1. NEVER WHEN THIS CLIENT IS THE OWNER. The owner's own resize is echoed
//      back to it, and bouncing on it would make every window drag reconnect
//      the person doing the dragging.
//   2. NEVER WHILE A TAKE-OVER IS ARMED. Take-over is itself a bounce carrying
//      an intent; a second `connect()` on top of it closes a socket that is
//      still opening and the claim rides nothing.
//   3. NEVER WHILE A BOUNCE IS ALREADY IN FLIGHT. Same reason, without the
//      intent.
//   4. NEVER FOR A PANE WITH NO SOCKET. A dormant tab is never mounted (App
//      renders its card instead, because subscribing force-launches), so there
//      is nothing to reconnect and nothing to heal.
//   5. THE HANDSHAKE'S OWN GRID NEVER TRIGGERS ONE. A fresh attach has just
//      rebuilt its buffer from the server's repaint; bouncing again on the
//      grid that attach reported would loop forever. Only a CHANGE after the
//      attach heals.
//
// DEBOUNCED, because a resize is a burst. The owner's own send is debounced at
// `RESIZE_SEND_DEBOUNCE_MS` and its first open jiggles the width twice 60ms
// apart, so one drag or one open produces several applied grids. The window
// here is deliberately longer than both, so a burst settles into exactly ONE
// bounce.
import { useEffect, useMemo, useRef, useState } from "react"

import type { PtySocket } from "@/lib/ptySocket"

import type { OwnershipVerdict, TakeoverIntent } from "./channels"
import { VIEWER_HEAL_DEBOUNCE_MS } from "./constants"

export type Grid = { rows: number; cols: number }

/// Whether a viewer is rendering at a geometry the child is not drawing for.
///
/// Pure, and deliberately conservative in both unknown directions: an unknown
/// remote grid (an old server, or a pty the server could not read) and an
/// unknown local one are both "nothing to claim", never "they disagree". A
/// badge shown on a guess is worse than no badge, because it is unfalsifiable
/// from the user's side.
export function gridsDiverge(local: Grid | null, remote: Grid | null): boolean {
  if (!local || !remote) return false
  return local.rows !== remote.rows || local.cols !== remote.cols
}

/// Whether an announced grid change is worth bouncing this socket for.
///
/// Split out from the hook so the five guards are readable as a table and
/// testable without a socket. `changed` is the caller's answer to "is this
/// different from the last remote grid we knew", which is what keeps a repeated
/// announcement of the same geometry from re-arming the timer forever.
export function shouldHealByReattaching(state: {
  isOwner: boolean
  takeoverArmed: boolean
  bounceInFlight: boolean
  hasSocket: boolean
  fromHandshake: boolean
  changed: boolean
}): boolean {
  if (state.isOwner) return false
  if (state.takeoverArmed) return false
  if (state.bounceInFlight) return false
  if (!state.hasSocket) return false
  if (state.fromHandshake) return false
  return state.changed
}

export type ViewerGridDeps = {
  ptyRef: { current: PtySocket | null }
  ownership: OwnershipVerdict
  takeoverIntent: TakeoverIntent
  /// The pane's reconnect cue, raised by hand exactly as the take-over bounce
  /// raises it: a deliberate `connect()` fires no `onReconnecting` of its own
  /// (see `ReconnectingSocket.connect`), so without this the healing window
  /// reads as a frozen terminal.
  setReconnecting: (value: boolean) => void
}

export type ViewerGrid = {
  /// The PTY's grid as the wire last reported it, or null when nothing has.
  remoteGrid: Grid | null
  /// This xterm's own grid, or null before the first fit.
  localGrid: Grid | null
  /// Record a grid the wire reported. `fromHandshake` distinguishes the attach
  /// snapshot from a later change; only a change can heal.
  noteRemoteGrid: (grid: Grid | null, fromHandshake: boolean) => void
  /// Record this xterm's grid. Called for the mount fit and from xterm's own
  /// resize event, which fires only when the grid really changed.
  noteLocalGrid: (grid: Grid) => void
  /// The socket opened, so any bounce this machine started has landed.
  noteSocketOpen: () => void
  /// Drop any armed heal. Called by the lifecycle teardown, so a bounce armed
  /// in one mount can never fire into the next one's socket.
  dispose: () => void
}

export function useViewerGrid(deps: ViewerGridDeps): ViewerGrid {
  const { ptyRef, ownership, takeoverIntent, setReconnecting } = deps

  const [remoteGrid, setRemoteGrid] = useState<Grid | null>(null)
  const [localGrid, setLocalGrid] = useState<Grid | null>(null)
  // The last grid the wire reported, read synchronously by the announcement
  // handler: two `size` events can land in one commit, and a state read would
  // still be showing the first one's value when the second arrives.
  const remoteRef = useRef<Grid | null>(null)
  const healTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  // Guard 3. Set when this machine calls `connect()` and cleared when the
  // socket opens, so a burst that outlives the debounce cannot stack bounces.
  const bouncing = useRef(false)

  const machine = useMemo<ViewerGrid>(() => {
    const clearHeal = () => {
      clearTimeout(healTimer.current)
      healTimer.current = undefined
    }
    return {
      // Overwritten below with this render's values; the object identity is
      // what has to stay stable, because the lifecycle closes over the
      // callbacks for the whole mount.
      remoteGrid: null,
      localGrid: null,
      noteRemoteGrid(grid, fromHandshake) {
        // "Different from what we already knew". A first grid where none was
        // known counts, and a repeat of the same geometry does not, which is
        // what stops a re-announcement from re-arming the timer forever.
        const previous = remoteRef.current
        const changed =
          grid !== null &&
          (previous === null ||
            previous.rows !== grid.rows ||
            previous.cols !== grid.cols)
        remoteRef.current = grid
        setRemoteGrid(grid)
        const heal = shouldHealByReattaching({
          isOwner: ownership.read(),
          takeoverArmed: takeoverIntent.read(),
          bounceInFlight: bouncing.current,
          hasSocket: ptyRef.current !== null,
          fromHandshake,
          changed,
        })
        if (!heal) return
        // Re-armed on every announcement in the burst, so the bounce lands once
        // the geometry has settled rather than once per intermediate size.
        clearHeal()
        healTimer.current = setTimeout(() => {
          healTimer.current = undefined
          // Re-check the LIVE guards at FIRING time through the same decision
          // table as arming, so the two can never drift: the debounce window
          // is long enough for the user to press Take over, or for a handover
          // to make this client the owner, and either makes the bounce wrong.
          // `fromHandshake` and `changed` are arming-time facts (a heal is
          // only ever armed by a non-handshake change), so they are passed as
          // the constants that armed it.
          const pty = ptyRef.current
          const fire = shouldHealByReattaching({
            isOwner: ownership.read(),
            takeoverArmed: takeoverIntent.read(),
            bounceInFlight: bouncing.current,
            hasSocket: pty !== null,
            fromHandshake: false,
            changed: true,
          })
          if (!fire || !pty) return
          bouncing.current = true
          setReconnecting(true)
          pty.connect()
        }, VIEWER_HEAL_DEBOUNCE_MS)
      },
      noteLocalGrid(grid) {
        setLocalGrid((prev) =>
          prev && prev.rows === grid.rows && prev.cols === grid.cols
            ? prev
            : grid,
        )
      },
      noteSocketOpen() {
        bouncing.current = false
        // An armed heal must not survive the open either: whatever bounce it
        // was scheduled for, this open has just rebuilt the buffer from the
        // server's repaint, so firing it now would be a redundant bounce at a
        // just-healed socket. An unrelated reconnect (a network blip, a
        // take-over) retires it for the same reason; the next real grid
        // change after this open arms a fresh one.
        clearHeal()
      },
      dispose() {
        clearHeal()
        bouncing.current = false
      },
    }
    // The channels and the refs are stable for the pane's lifetime, and
    // `setReconnecting` is a setState. Listing them would rebuild the machine
    // the lifecycle has already closed over.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => () => machine.dispose(), [machine])

  return { ...machine, remoteGrid, localGrid }
}
