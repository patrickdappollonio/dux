// THE TOUCH-GESTURE MACHINE: the SOLE owner of touch disambiguation.
//
// Every finger on the terminal is one of exactly three things, and this decides
// which, once, for everybody:
//
//   a one-finger DRAG scrolls (the scrollback locally, or the app's own history
//   as forwarded wheel notches),
//   a stationary LONG PRESS selects the word under the finger, and the drag
//   after it extends the selection,
//   a quick TAP is neither, and falls through to its client (which focuses the
//   compose box, probes for a hyperlink, or lets xterm have it).
//
// THE DISAMBIGUATION. A long-press timer marks the gesture as a SELECTION the
// moment the finger has been held still past the delay; from then on it never
// scrolls, and every move re-selects instead. If the finger instead MOVES past
// a small threshold before that fires, it is a scroll, so the timer is
// cancelled and the scroll takes over. A short, still tap trips neither.
//
// ANY SECOND FINGER CANCELS THE WHOLE GESTURE, not just the pending timer:
// leaving the selecting flag set meant lifting one finger out of a PINCH took
// the selecting branch and copied. The painted selection is left alone, since
// the user can still see it and may be pinching in order to read it.
//
// It EMITS EVENTS and does no work of its own. The selection drag, the resize
// hold, the wheel forwarding and the tap redirect are all clients, which is
// what keeps "is this a scroll or a selection" answered in exactly one place
// instead of being re-derived from flags at each site.
export type TouchGestureOutcome = {
  /// Neither a scroll nor a long press: the gesture the tap redirect acts on.
  wasTap: boolean
  /// The long press fired, so this lift ends a SELECTION. Deliberately not a
  /// tap: the lift must not focus anything or raise the keyboard over the text
  /// the user has just selected.
  wasSelecting: boolean
}

export type TouchGestureClient = {
  /// May a move drive a scroll at all right now? Asked FRESH on every move,
  /// because an agent can flip in or out of an alt-screen TUI mid-drag.
  scrollAllowed: () => boolean
  /// Drop whatever the previous gesture was holding: the selection anchor and
  /// the resize hold. Fires on every new touch and on every ending.
  onGestureReset: () => void
  /// The gesture is over (a lift or a cancel): release what it held back.
  onGestureFinished: () => void
  /// The long-press timer fired. The gesture is a selection from here on
  /// WHATEVER the press landed on: a press on blank space selects nothing, but
  /// it is still not a tap.
  onLongPress: (touch: Touch) => void
  /// The finger moved while selecting.
  onSelectMove: (touch: Touch) => void
  /// The finger has clearly moved: this is a scroll. Fires once per gesture.
  onScrollStart: () => void
  /// The finger moved while scrolling, carrying the accumulated pixels.
  /// Returns the accumulator to carry into the next move (the remainder left
  /// over after whole rows were consumed).
  onScrollMove: (accumPx: number, touch: Touch) => number
  /// The lift, with what the gesture turned out to be.
  onLift: (outcome: TouchGestureOutcome, e: TouchEvent) => void
}

/// How long a still finger must be held before the gesture becomes a selection.
export const LONG_PRESS_MS = 400
/// How far a finger must travel before the gesture becomes a scroll.
export const SCROLL_THRESHOLD_PX = 8

export type TouchGesture = {
  /// Register on the container. Touch-only listeners, so this also lights up a
  /// touchscreen laptop, not just the mobile layout. `touchend` is registered
  /// NON-PASSIVE unconditionally (even where the tap redirect never fires): a
  /// deliberate, harmless choice, since touchend passivity does not gate the
  /// browser's scroll optimizations the way touchmove's does.
  attach: (container: Element) => void
  dispose: () => void
}

export function createTouchGesture(client: TouchGestureClient): TouchGesture {
  let lastY = 0
  let accum = 0
  let scrolling = false
  let active = false
  let selecting = false
  let longPressTimer: ReturnType<typeof setTimeout> | undefined
  let attached: Element | null = null

  const onTouchStart = (e: TouchEvent) => {
    // Any new touch (including a second finger landing mid-gesture) supersedes
    // a pending long press, so always cancel it first.
    clearTimeout(longPressTimer)
    // Track single-finger touches on BOTH buffers: the normal buffer scrolls
    // xterm's scrollback, the alt-screen may forward to the app (decided per
    // move, since mouse-tracking state can change mid-gesture).
    if (e.touches.length !== 1) {
      // A pinch, or a second finger landing on an ACTIVE selection.
      active = false
      scrolling = false
      selecting = false
      client.onGestureReset()
      return
    }
    active = true
    scrolling = false
    selecting = false
    client.onGestureReset()
    accum = 0
    lastY = e.touches[0].clientY
    const start = e.touches[0]
    longPressTimer = setTimeout(() => {
      selecting = true
      client.onLongPress(start)
    }, LONG_PRESS_MS)
  }

  const onTouchMove = (e: TouchEvent) => {
    if (!active || e.touches.length !== 1) return
    if (selecting) {
      // Ours now: keep the page from scrolling under the gesture and grow the
      // span instead. Never falls through to the scroll path.
      e.preventDefault()
      client.onSelectMove(e.touches[0])
      return
    }
    // Asked fresh each move, so an app flipping into a full-screen TUI with no
    // mouse tracking stops the drag mid-gesture rather than scrolling a buffer
    // that has no scrollback.
    if (!client.scrollAllowed()) return
    const y = e.touches[0].clientY
    accum += y - lastY
    lastY = y
    // Engage only once the finger has clearly moved, so a tap or an
    // about-to-be long press is never stolen.
    if (!scrolling && Math.abs(accum) < SCROLL_THRESHOLD_PX) return
    if (!scrolling) {
      // Movement won the race against the long press: this is a scroll.
      clearTimeout(longPressTimer)
      scrolling = true
      client.onScrollStart()
    }
    e.preventDefault()
    accum = client.onScrollMove(accum, e.touches[0])
  }

  const reset = () => {
    clearTimeout(longPressTimer)
    active = false
    scrolling = false
    selecting = false
    client.onGestureReset()
    client.onGestureFinished()
  }

  const onTouchEnd = (e: TouchEvent) => {
    const outcome: TouchGestureOutcome = {
      wasTap: active && !scrolling && !selecting,
      wasSelecting: selecting,
    }
    reset()
    client.onLift(outcome, e)
  }

  // A cancel is an ending with no lift: the gesture releases what it held, and
  // nothing acts on what it might have been.
  const onTouchCancel = () => reset()

  return {
    attach(container) {
      attached = container
      container.addEventListener("touchstart", onTouchStart as EventListener, {
        passive: true,
      })
      container.addEventListener("touchmove", onTouchMove as EventListener, {
        passive: false,
      })
      container.addEventListener("touchend", onTouchEnd as EventListener, {
        passive: false,
      })
      container.addEventListener("touchcancel", onTouchCancel as EventListener, {
        passive: true,
      })
    },
    dispose() {
      // Finish any in-flight gesture first: reset() releases the resize hold
      // and tells the selection client the gesture is over, which is what
      // stops the 50ms edge auto-scroll interval. Without it, an unmount
      // mid-selection left that interval firing against a disposed terminal
      // (harmless only by accident: the first post-dispose tick read a null
      // screen rect and stopped itself).
      reset()
      clearTimeout(longPressTimer)
      attached?.removeEventListener("touchstart", onTouchStart as EventListener)
      attached?.removeEventListener("touchmove", onTouchMove as EventListener)
      attached?.removeEventListener("touchend", onTouchEnd as EventListener)
      attached?.removeEventListener(
        "touchcancel",
        onTouchCancel as EventListener,
      )
      attached = null
    },
  }
}
