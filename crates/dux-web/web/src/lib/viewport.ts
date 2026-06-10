// Small pure helpers for mobile terminal viewport geometry — detecting the soft
// keyboard (`keyboardLikelyOpen`) and converting a touch drag into terminal
// scroll lines (`dragScrollLines`). Kept pure so both unit-test without a DOM.

// Heuristic for "is the soft keyboard open" on mobile. The keyboard shrinks the
// VISUAL viewport (window.visualViewport.height) but not the LAYOUT viewport
// (window.innerHeight), so a large gap between the two means the keyboard is up.
//
// The threshold sits ABOVE the iOS dynamic-toolbar (URL bar) collapse delta
// (~60-90px, which must NOT be mistaken for a keyboard) and BELOW the smallest
// real soft keyboard (~120px+ including its accessory row). 100px threads that
// gap. Tune here if a device misreports; pure so it can be unit-tested.
export const KEYBOARD_OPEN_THRESHOLD_PX = 100

export function keyboardLikelyOpen(
  viewportHeight: number,
  innerHeight: number,
): boolean {
  return innerHeight - viewportHeight > KEYBOARD_OPEN_THRESHOLD_PX
}

// Convert an accumulated one-finger vertical drag (px, positive = downward) into
// arguments for xterm's `scrollLines()`: how many lines to scroll now, plus the
// leftover sub-row pixels to carry into the next move so a slow drag still
// scrolls smoothly instead of snapping a whole row at a time.
//
// Natural scrolling: dragging DOWN (positive px) pulls the content down, which
// reveals OLDER output — `scrollLines()` with a NEGATIVE argument — so the sign
// flips. `rowHeight` falls back to a sane non-zero value so a transient
// zero-height measurement can never divide by zero or scroll infinitely. Pure so
// it can be unit-tested; the touch handler owns the event plumbing.
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
