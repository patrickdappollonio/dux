import * as React from "react"

import {
  DIVIDER_TARGET_MIN,
  dividerCursor,
  dividerHitBand,
  withinDividerBand,
} from "@/lib/paneDivider"
import { useIsCoarsePointer } from "@/hooks/use-coarse-pointer"

// The drag half of the shared divider mechanism, for the dividers dux drives
// itself. It is a deliberate reimplementation of what `react-resizable-panels`
// does for the Changes separator, so the two gestures feel the same:
//
//   - the press is acquired on the DOCUMENT, in the capture phase, by testing
//     the pointer against the divider's grab band rather than by hit-testing
//     the element, so nothing painted over the divider can swallow it;
//   - non-primary mouse buttons are ignored, a second pointer arriving mid-drag
//     is ignored, the divider takes focus without a focus ring and without
//     scrolling, and the press is default-prevented;
//   - the move is a DELTA from the press point, so the divider does not
//     teleport under a finger that landed off centre;
//   - a mouse that reports no buttons held ends the gesture, because a
//     `pointerup` delivered to another window never reaches us;
//   - the resize cursor is painted over the whole document while the band is
//     hovered or dragged, and is dropped when the pointer leaves the document;
//   - a double-click inside the band resets the divider.
//
// Everything above is measured against react-resizable-panels 4.11.2.

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

/**
 * Wire an element up as a draggable divider. Returns the ref to put on it.
 */
export function useDividerDrag(
  handlers: DividerDragHandlers,
): React.RefObject<HTMLDivElement | null> {
  const ref = React.useRef<HTMLDivElement | null>(null)
  const coarse = useIsCoarsePointer()

  // The handlers are re-created on every render of the owning component (they
  // close over live state), so they are read through a ref and the listeners
  // below are installed once.
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

    const bandFor = () => {
      const el = ref.current
      if (!el) return null
      return dividerHitBand(el.getBoundingClientRect(), minWidth)
    }

    const hits = (event: PointerEvent | MouseEvent) => {
      const band = bandFor()
      return (
        band !== null && withinDividerBand(band, event.clientX, event.clientY)
      )
    }

    const release = () => {
      activePointer = null
      claimCursor(owner, false)
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
      el?.focus({ preventScroll: true })
      el?.setPointerCapture?.(event.pointerId)
      claimCursor(owner, true)
    }

    const onPointerMove = (event: PointerEvent) => {
      if (activePointer === null) {
        // Not dragging: the document cursor follows the band, the way the
        // panel library's does, so the splitter cursor appears before the
        // pointer reaches the hair-thin line.
        claimCursor(owner, event.pointerType === "mouse" && hits(event))
        return
      }
      if (event.pointerId !== activePointer) return
      // THE LOST POINTERUP. A release over another window, over browser chrome,
      // or swallowed by a native drag never reaches this document, and the
      // divider would then follow the mouse with nothing held down. The first
      // move that reports no buttons ends the gesture where it stands, which is
      // what the library does for the same reason.
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

    // Capture can be lost without a pointerup: the element is removed, or the
    // browser hands the pointer to something else. Either way the gesture is
    // over, and the divider must not keep following the pointer.
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
