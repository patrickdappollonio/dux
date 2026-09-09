// A PTY has one authoritative grid, the owner's. A non-owner renders the same
// byte stream into a differently sized xterm, so every repaint wraps mangled
// rows into that viewer's local scrollback. It heals by bouncing its own
// socket, never by resizing the PTY (that would be a silent steal): the
// reconnect path rebuilds the buffer from the server's repaint at the new
// geometry. The bounce is debounced longer than `RESIZE_SEND_DEBOUNCE_MS` so a
// drag's burst of applied grids settles into one bounce.
import { useEffect, useMemo, useRef, useState } from "react"

import type { PtySocket } from "@/lib/ptySocket"

import type { OwnershipVerdict, TakeoverIntent } from "./channels"
import { plainBounce } from "./plainBounce"
import { VIEWER_HEAL_DEBOUNCE_MS } from "./constants"

export type Grid = { rows: number; cols: number }

/// Whether two grids disagree, and the one definition of divergence the heal's
/// "is this announcement a change" check runs through.
///
/// Conservative in both unknown directions: an unknown remote grid (an old
/// server, or a pty the server could not read) and an unknown local one are
/// both "nothing to claim", never "they disagree".
export function gridsDiverge(local: Grid | null, remote: Grid | null): boolean {
  if (!local || !remote) return false
  return local.rows !== remote.rows || local.cols !== remote.cols
}

/// Whether an announced grid change is worth bouncing this socket for. Each
/// guard is a way the bounce would be wrong rather than merely wasteful:
///   isOwner: the owner's own resize is echoed back, so every drag would bounce.
///   takeoverArmed, bounceInFlight: closes a socket that is still opening.
///   hasSocket: a dormant tab is never mounted, so there is nothing to heal.
///   fromHandshake: that attach just rebuilt the buffer, so it would loop.
///   changed: a re-announcement of the same geometry must not re-arm the timer.
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
  /// The pane's reconnect cue, raised by hand: a deliberate `connect()` fires
  /// no `onReconnecting`, so without this the heal reads as a frozen terminal.
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
  // Set when this machine calls `connect()` and cleared when the socket opens,
  // so a burst that outlives the debounce cannot stack bounces.
  const bouncing = useRef(false)

  const machine = useMemo<ViewerGrid>(() => {
    const clearHeal = () => {
      clearTimeout(healTimer.current)
      healTimer.current = undefined
    }
    return {
      // Overwritten below with this render's values; the object identity must
      // stay stable, because the lifecycle closes over the callbacks.
      remoteGrid: null,
      localGrid: null,
      noteRemoteGrid(grid, fromHandshake) {
        // A first grid where none was known counts as a change; a repeat of the
        // same geometry does not, so it cannot re-arm the timer forever.
        const previous = remoteRef.current
        const changed =
          grid !== null && (previous === null || gridsDiverge(previous, grid))
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
          // Re-check the live guards at firing time: the debounce window is
          // long enough for a take-over or a handover to make the bounce wrong.
          // `fromHandshake` and `changed` are arming-time facts, so they are
          // passed as the constants that armed the heal.
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
          // Through the one plain-bounce helper, like every reopen that is not
          // a take-over, so no bounce grows its own answer to what happens to
          // an unspent claim.
          plainBounce(pty, takeoverIntent)
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
        // Any open, this machine's or not, has just rebuilt the buffer from the
        // server's repaint, so an armed heal would be a redundant bounce at a
        // healed socket. The next grid change after this open arms a fresh one.
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
