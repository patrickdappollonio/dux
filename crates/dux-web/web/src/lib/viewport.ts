// Pure helpers for mobile terminal viewport geometry: soft-keyboard detection and turning a
// touch drag into terminal scroll lines.

// The soft keyboard shrinks the visual viewport but not the layout viewport, so a large gap
// between the two means it is up. The threshold sits above the iOS URL-bar collapse delta
// (~60-90px, which must not read as a keyboard) and below the smallest real keyboard (~120px).
export const KEYBOARD_OPEN_THRESHOLD_PX = 100

export function keyboardLikelyOpen(
  viewportHeight: number,
  innerHeight: number,
): boolean {
  return innerHeight - viewportHeight > KEYBOARD_OPEN_THRESHOLD_PX
}

// Convert an accumulated one-finger vertical drag (px, positive downward) into arguments for
// xterm's `scrollLines()`, plus the leftover sub-row pixels to carry into the next move so a
// slow drag scrolls smoothly. Natural scrolling: dragging down reveals older output, so the
// sign flips. `rowHeight` falls back to a non-zero value, so a transient zero-height
// measurement can neither divide by zero nor scroll infinitely.
export function dragScrollLines(
  accumPx: number,
  rowHeight: number,
): { scrollLines: number; remainderPx: number } {
  const h = rowHeight > 0 ? rowHeight : 16
  const whole = Math.trunc(accumPx / h)
  return {
    // `whole === 0 ? 0` avoids returning a negated zero (`-0`) for sub-row drags.
    scrollLines: whole === 0 ? 0 : -whole,
    remainderPx: accumPx - whole * h,
  }
}

// Convert an accumulated drag into a single wheel notch to forward to a mouse-tracking
// alt-screen app, rather than scrolling locally. The notch is capped at magnitude one per
// touch-move, reproducing a physical wheel's one-report-per-tick cadence: forwarding the raw
// magnitude emits a dense burst of reports in one frame, which corrupts an alt-screen pager's
// repaint. The whole rows the finger travelled are still consumed, so the accumulator never
// grows and successive moves keep tracking the finger.
export function dragWheelReport(
  accumPx: number,
  rowHeight: number,
): { notch: number; remainderPx: number } {
  const { scrollLines, remainderPx } = dragScrollLines(accumPx, rowHeight)
  // `Math.sign` collapses any multi-row magnitude to -1, 0, or +1 while keeping
  // the drag direction the local path uses.
  return { notch: Math.sign(scrollLines), remainderPx }
}
