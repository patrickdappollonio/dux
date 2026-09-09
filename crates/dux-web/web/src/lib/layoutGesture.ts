// One PTY refit per layout gesture. An animated layout change moves the terminal's
// host box on every frame, and a measured xterm resize resets the scrolling region on
// both buffers, corrupting a pager still repainting for the old geometry. So the
// gesture holds the coordinator for its whole duration and releases once at the end,
// coalescing every frame into one fit and one resize frame on the wire.
//
// A module-level registry because the surface running the gesture is the pane's
// sibling while the coordinator is private to the pane's lifecycle closure.
//
// The depth counter is for overlapping gestures: the hold is taken on the way past
// zero and released on the way back, so a second toggle cannot release the first
// one's hold. The touch-scroll wiring takes the same hold directly, so a gesture
// begun during a live touch scroll can be released early, costing one extra fit.

/** What a mounted pane offers the gesture: take the hold, and let it go. */
export interface LayoutGestureHolder {
  hold: () => void
  release: () => void
}

const holders = new Set<LayoutGestureHolder>()
let depth = 0

/** Register a mounted pane's hold. Returns the unregister. */
export function registerLayoutGestureHolder(
  holder: LayoutGestureHolder,
): () => void {
  holders.add(holder)
  // A pane mounting inside a running gesture takes the hold immediately, or its
  // first frames fit freely and the gesture's guarantee is about the wrong pane.
  if (depth > 0) holder.hold()
  return () => {
    holders.delete(holder)
  }
}

/** Begin a layout gesture: every mounted pane stops fitting until it ends. */
export function beginLayoutGesture(): void {
  depth += 1
  if (depth !== 1) return
  for (const holder of [...holders]) holder.hold()
}

/** End a layout gesture, which is where the one refit happens. */
export function endLayoutGesture(): void {
  if (depth === 0) return
  depth -= 1
  if (depth !== 0) return
  for (const holder of [...holders]) holder.release()
}

/** A gesture in flight: it can be re-armed, and it can be ended early. */
export interface LayoutGestureHandle {
  /**
   * Re-arms the window without letting the hold go, which is what a re-toggle
   * mid-transition needs: releasing now would fit at a geometry the layout is only
   * passing through. A restart after the window closed is a no-op.
   */
  restart: (durationMs: number) => void
  /** End the gesture now, which releases the hold and pays for the one fit. */
  cancel: () => void
}

/**
 * Holds the layout for `durationMs`, then releases; the caller animates inside that
 * window and the release pays for the refit. A zero duration still takes the hold, so
 * the reduced-motion path has the same shape and the same single refit.
 *
 * `onEnd` fires exactly once, whichever way the window closed.
 */
export function holdLayoutForGesture(
  durationMs: number,
  onEnd?: () => void,
): LayoutGestureHandle {
  beginLayoutGesture()
  let ended = false
  const finish = () => {
    if (ended) return
    ended = true
    endLayoutGesture()
    onEnd?.()
  }
  let timer = setTimeout(finish, Math.max(0, durationMs))
  return {
    restart: (ms: number) => {
      if (ended) return
      clearTimeout(timer)
      timer = setTimeout(finish, Math.max(0, ms))
    },
    cancel: () => {
      clearTimeout(timer)
      finish()
    },
  }
}

/** Test-only: how many gestures are in flight. */
export function layoutGestureDepth(): number {
  return depth
}
