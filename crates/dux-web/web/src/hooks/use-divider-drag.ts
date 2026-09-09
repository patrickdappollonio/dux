import * as React from "react"

import {
  DIVIDER_HELD_ATTR,
  DIVIDER_HELD_OFF,
  DIVIDER_HELD_ON,
  DIVIDER_TARGET_MIN,
  dividerCursor,
  dividerPressHits,
} from "@/lib/paneDivider"
import { beginLayoutGesture, endLayoutGesture } from "@/lib/layoutGesture"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"

// The drag half of the shared divider mechanism, for the dividers dux drives
// itself, deliberately mirroring what react-resizable-panels 4.11.2 does for the
// Changes separator so the two gestures feel the same. The press is acquired on
// the document, in the capture phase, by testing the pointer against the
// divider's grab band rather than by hit-testing the element, so nothing painted
// over the divider can swallow it; the move is a delta from the press point, so
// the divider does not teleport under a finger that landed off centre.

export interface DividerDragHandlers {
  /** The press was accepted; snapshot whatever the delta will be measured from. */
  onGrab?: () => void
  /** Live during the drag; `deltaX` is signed pixels from the press point. */
  onDrag: (deltaX: number) => void
  /** End of a completed gesture, with the final delta. */
  onDrop: (deltaX: number) => void
  /** The browser took the gesture away; nothing should be committed. */
  onCancel?: () => void
  /** Double-click inside the band. */
  onReset?: () => void
}

const CURSOR_STYLE_ID = "dux-divider-cursor"

// One document-wide cursor rule, shared by every divider that asks for it, so
// two dividers hovered in sequence cannot leave a stale rule behind.
const cursorClaims = new Set<symbol>()

function syncCursorStyle() {
  const existing = document.getElementById(CURSOR_STYLE_ID)
  if (cursorClaims.size === 0) {
    existing?.remove()
    return
  }
  const cursor = dividerCursor(navigator.userAgent)
  const rule = `*, *:hover { cursor: ${cursor} !important; }`
  if (existing) {
    if (existing.textContent !== rule) existing.textContent = rule
    return
  }
  const style = document.createElement("style")
  style.id = CURSOR_STYLE_ID
  style.textContent = rule
  document.head.append(style)
}

function claimCursor(owner: symbol, claimed: boolean) {
  if (claimed) cursorClaims.add(owner)
  else cursorClaims.delete(owner)
  syncCursorStyle()
}

// The held paint, written by dux on both dividers. The attribute is always
// present rather than added and removed, so a parity test can tell which
// elements take part in the paint. See DIVIDER_HELD_ATTR.
function paintHeld(el: HTMLElement | null, held: boolean) {
  el?.setAttribute(DIVIDER_HELD_ATTR, held ? DIVIDER_HELD_ON : DIVIDER_HELD_OFF)
}

/**
 * Publish the held paint on a divider dux does not drive: the Changes pane's
 * separator, which react-resizable-panels drags itself. It paints and brackets
 * and nothing else, so the drag, the layout and the persistence stay with the
 * library; acquisition is the shared `dividerPressHits` against the same grab
 * band the library claims, so both dividers light on the same presses.
 *
 * dux watches the pointer for a gesture it does not own because 4.11.2 has no
 * `pointercancel` listener: a touch the browser takes away leaves the library's
 * own `data-separator` latched at `active`, and a paint keyed on that attribute
 * stays lit until the next press.
 *
 * The bracket is the layout gesture. The terminal pane's debounce coalesces only
 * the moves inside one quiet window, so a finger that pauses mid-drag would buy
 * a refit at a geometry it is passing through; holding from press to release
 * pays for one fit at the width the drag settles on.
 */
export function useDividerHeld(): React.RefObject<HTMLDivElement | null> {
  const ref = React.useRef<HTMLDivElement | null>(null)
  const coarse = useIsCoarsePointer()

  React.useEffect(() => {
    const minWidth = coarse
      ? DIVIDER_TARGET_MIN.coarse
      : DIVIDER_TARGET_MIN.fine
    let heldPointer: number | null = null
    let holding = false

    const hold = () => {
      if (holding) return
      holding = true
      beginLayoutGesture()
    }
    const unhold = () => {
      if (!holding) return
      holding = false
      endLayoutGesture()
    }

    const onPointerDown = (event: PointerEvent) => {
      // Deliberately not gated on `defaultPrevented`: the library's own document
      // listener runs in the same capture phase and may already have claimed the
      // press, and this one only paints.
      if (heldPointer !== null) return
      if (event.pointerType === "mouse" && event.button > 0) return
      if (!dividerPressHits(ref.current, event, minWidth)) return
      heldPointer = event.pointerId
      paintHeld(ref.current, true)
      hold()
    }

    const onPointerEnd = (event: PointerEvent) => {
      if (heldPointer === null || event.pointerId !== heldPointer) return
      heldPointer = null
      paintHeld(ref.current, false)
      unhold()
    }

    const el = ref.current
    paintHeld(el, false)
    document.addEventListener("pointerdown", onPointerDown, true)
    document.addEventListener("pointerup", onPointerEnd, true)
    document.addEventListener("pointercancel", onPointerEnd, true)
    // A capture lost without a pointerup ends the gesture too: the element goes
    // away, or the browser hands the pointer to something else.
    el?.addEventListener("lostpointercapture", onPointerEnd as EventListener)
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true)
      document.removeEventListener("pointerup", onPointerEnd, true)
      document.removeEventListener("pointercancel", onPointerEnd, true)
      el?.removeEventListener(
        "lostpointercapture",
        onPointerEnd as EventListener,
      )
      // A hold must not outlive the listeners that would have released it.
      unhold()
    }
  }, [coarse])

  return ref
}

/**
 * Wire an element up as a draggable divider. Returns the ref to put on it.
 */
export function useDividerDrag(
  handlers: DividerDragHandlers,
): React.RefObject<HTMLDivElement | null> {
  const ref = React.useRef<HTMLDivElement | null>(null)
  const coarse = useIsCoarsePointer()

  // The handlers close over live state and are re-created on every render of the
  // owning component, so they are read through a ref and the listeners below are
  // installed once.
  const handlersRef = React.useRef(handlers)
  React.useEffect(() => {
    handlersRef.current = handlers
  })

  React.useEffect(() => {
    const owner = Symbol("divider")
    const minWidth = coarse
      ? DIVIDER_TARGET_MIN.coarse
      : DIVIDER_TARGET_MIN.fine
    let activePointer: number | null = null
    let startX = 0
    let lastDeltaX = 0

    // Which presses belong to this divider: the shared rule, so the two
    // dividers cannot disagree about what counts as landing on one.
    const hits = (event: PointerEvent | MouseEvent) =>
      dividerPressHits(ref.current, event, minWidth)

    const paintState = (active: boolean) => {
      paintHeld(ref.current, active)
    }

    const release = () => {
      activePointer = null
      claimCursor(owner, false)
      paintState(false)
    }

    const finish = () => {
      const delta = lastDeltaX
      release()
      handlersRef.current.onDrop(delta)
    }

    const onPointerDown = (event: PointerEvent) => {
      if (event.defaultPrevented) return
      // A second finger arriving mid-drag is not a second divider gesture. The
      // first pointer owns the divider until it ends.
      if (activePointer !== null) return
      if (event.pointerType === "mouse" && event.button > 0) return
      if (!hits(event)) return
      event.preventDefault()
      activePointer = event.pointerId
      startX = event.clientX
      lastDeltaX = 0
      handlersRef.current.onGrab?.()
      const el = ref.current
      // `focusVisible: false`, as react-resizable-panels 4.11.2 asks for it on
      // its own separator: focus still moves, so a drag can be carried on from
      // the keyboard, but a press leaves no focus ring beside the line.
      el?.focus({ preventScroll: true, focusVisible: false })
      el?.setPointerCapture?.(event.pointerId)
      claimCursor(owner, true)
      paintState(true)
    }

    const onPointerMove = (event: PointerEvent) => {
      if (activePointer === null) {
        // Not dragging: the document cursor follows the band, the way the panel
        // library's does, so it appears before the pointer reaches the line.
        claimCursor(owner, event.pointerType === "mouse" && hits(event))
        return
      }
      if (event.pointerId !== activePointer) return
      // A release over another window, over browser chrome, or swallowed by a
      // native drag never reaches this document, so the first move that reports
      // no buttons ends the gesture where it stands, as the library does.
      if (event.pointerType === "mouse" && event.buttons === 0) {
        finish()
        return
      }
      lastDeltaX = event.clientX - startX
      handlersRef.current.onDrag(lastDeltaX)
    }

    const onPointerUp = (event: PointerEvent) => {
      if (activePointer === null || event.pointerId !== activePointer) return
      lastDeltaX = event.clientX - startX
      finish()
    }

    const onPointerCancel = (event: PointerEvent) => {
      if (activePointer === null || event.pointerId !== activePointer) return
      release()
      handlersRef.current.onCancel?.()
    }

    // Capture can be lost without a pointerup (the element is removed, or the
    // browser hands the pointer elsewhere); the gesture is over either way.
    const onLostCapture = (event: PointerEvent) => {
      if (activePointer === null || event.pointerId !== activePointer) return
      finish()
    }

    // The pointer left the document entirely, so nothing is hovered any more.
    // Without this the resize cursor stays claimed over the whole page.
    const onPointerLeave = () => {
      if (activePointer !== null) return
      claimCursor(owner, false)
    }

    const onDoubleClick = (event: MouseEvent) => {
      if (event.defaultPrevented || !handlersRef.current.onReset) return
      if (!hits(event)) return
      event.preventDefault()
      handlersRef.current.onReset()
    }

    const el = ref.current
    paintState(false)
    document.addEventListener("pointerdown", onPointerDown, true)
    document.addEventListener("pointermove", onPointerMove, true)
    document.addEventListener("pointerup", onPointerUp, true)
    document.addEventListener("pointercancel", onPointerCancel, true)
    document.addEventListener("pointerleave", onPointerLeave, true)
    document.addEventListener("dblclick", onDoubleClick, true)
    el?.addEventListener("lostpointercapture", onLostCapture as EventListener)
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true)
      document.removeEventListener("pointermove", onPointerMove, true)
      document.removeEventListener("pointerup", onPointerUp, true)
      document.removeEventListener("pointercancel", onPointerCancel, true)
      document.removeEventListener("pointerleave", onPointerLeave, true)
      document.removeEventListener("dblclick", onDoubleClick, true)
      el?.removeEventListener(
        "lostpointercapture",
        onLostCapture as EventListener,
      )
      release()
    }
  }, [coarse])

  return ref
}
