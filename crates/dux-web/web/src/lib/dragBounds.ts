import type { Modifier } from "@dnd-kit/core"

interface Box {
  top: number
  left: number
  right: number
  bottom: number
}

// dnd-kit's autoscroll follows the pointer: it scrolls while the pointer is
// near an edge of the scroller, and stops only when the scroller itself reports
// that it is at its end. A row dragged by its own transform extends the
// scroller's scrollable overflow, so each scroll step moves that end further
// away and it is never reached: the list scrolls away for as long as the pointer
// is held outside it. Keeping the dragged box inside the scroller's own box
// leaves the content's real extent as the only end there is, so a drag near an
// edge still autoscrolls until the list's real end.

function clampAxis(delta: number, start: number, end: number, low: number, high: number): number {
  const min = low - start
  const max = high - end
  // A box larger than the bounds cannot fit: it keeps its leading edge in.
  if (min > max) return min
  return Math.min(Math.max(delta, min), max)
}

/** `transform` moved the least that puts `rect`, shifted by it, inside `bounds`. */
export function clampToBounds<T extends { x: number; y: number }>(
  transform: T,
  rect: Box,
  bounds: Box,
): T {
  return {
    ...transform,
    x: clampAxis(transform.x, rect.left, rect.right, bounds.left, bounds.right),
    y: clampAxis(transform.y, rect.top, rect.bottom, bounds.top, bounds.bottom),
  }
}

/** Keeps the dragged box inside the first ancestor that scrolls. */
export const restrictToScrollContainer: Modifier = ({
  transform,
  draggingNodeRect,
  scrollableAncestorRects,
}) => {
  const bounds = scrollableAncestorRects[0]
  if (!draggingNodeRect || !bounds) return transform
  return clampToBounds(transform, draggingNodeRect, bounds)
}

export const SORT_MODIFIERS: Modifier[] = [restrictToScrollContainer]
