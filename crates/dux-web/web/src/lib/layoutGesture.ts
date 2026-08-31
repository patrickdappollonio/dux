// ONE PTY REFIT PER LAYOUT GESTURE.
//
// A deliberate layout change that ANIMATES (theater mode taking the chrome
// away, and bringing it back) moves the terminal's host box on every frame of
// the transition. The pane's ResizeObserver would answer each of those frames
// with a fit, and a measured xterm resize resets the scrolling region on both
// buffers: a pager repainting for the old geometry is corrupted by a re-grid
// landing mid-repaint, and the refit and the child's resize notification are
// supposed to be one atomic pair. So the gesture takes the coordinator's
// existing hold for its whole duration and releases it once at the end, which
// coalesces every frame into exactly one fit at the geometry the gesture
// settled on, followed by one resize frame on the wire.
//
// A module-level registry, the same idiom as `setActivePtySocket` and
// `terminalFocus.ts`: the surface running the gesture lives outside the pane
// (the chrome that is leaving is the pane's SIBLING), and the coordinator is
// private to the pane's lifecycle closure, so the two meet here rather than
// through a prop chain that would have to cross both shells.
//
// The depth counter exists because gestures can overlap: a second toggle
// pressed mid-transition must not release a hold the first one still needs.
// The hold is taken on the way past zero and released on the way back, so
// overlapping gestures still cost exactly one fit between them.
//
// One stated cost: the touch-scroll wiring takes the SAME hold on the
// coordinator directly, so a layout gesture started in the middle of a live
// touch scroll can have its hold released early by the scroll's own flush.
// Reaching that needs a button press during a finger drag on the terminal, and
// the failure is a single extra fit, not a wrong one.

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
  // A pane that mounts INSIDE a running gesture takes the hold immediately;
  // otherwise its first frames would fit freely and the gesture's guarantee
  // would be about the wrong pane.
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
   * Re-arm the window WITHOUT letting the hold go.
   *
   * This is what a re-toggle mid-transition needs: the layout is still moving,
   * so releasing now would fit the terminal at a geometry it is passing
   * through rather than the one it settles on. A restart after the window has
   * already closed is a no-op, so a stale handle cannot take a hold nobody
   * will release.
   */
  restart: (durationMs: number) => void
  /** End the gesture now, which releases the hold and pays for the one fit. */
  cancel: () => void
}

/**
 * Hold the layout for `durationMs`, then release. The caller runs its own
 * animation inside that window; the release is what pays for the refit.
 *
 * A zero duration still goes through the hold rather than skipping it, so the
 * reduced-motion path and the animated one have the same shape and the same
 * single refit, one turn of the event loop apart.
 *
 * `onEnd` fires exactly once, whichever way the window closed, so the caller
 * can drop its handle rather than restarting a gesture that is already over.
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
