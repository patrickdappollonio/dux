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

// Convert an accumulated drag into a SINGLE wheel notch to FORWARD to a
// mouse-tracking alt-screen app (Claude Code, Codex, ...), as opposed to the
// local `scrollLines()` path above.
//
// The difference matters: a physical mouse wheel delivers ONE report per
// discrete wheel event, spaced across event-loop ticks, and xterm forwards it
// 1:1 (see `WHEEL_SCROLL_SENSITIVITY`'s note in TerminalPane). A finger drag,
// by contrast, can cover many rows in a single touch-move; forwarding that as
// `sgrWheelSeq(scrollLines, ..)` emits a DENSE burst of N reports inside one
// WebSocket frame with zero inter-notch spacing. That burst is what corrupted
// the agent's scrollback-pager repaint on a fast flick (duplicated lines that
// persist, since an alt-screen app has no client-side scrollback and nothing
// reconnects to re-sync the view). So we CAP the forwarded notch to magnitude
// one per touch-move, reproducing the desktop wheel's 1:1-per-tick cadence,
// while still consuming the whole rows the finger travelled so the accumulator
// never grows and successive moves keep tracking the finger. Pure so it can be
// unit-tested; the touch handler owns the event plumbing.
export function dragWheelReport(
  accumPx: number,
  rowHeight: number,
): { notch: number; remainderPx: number } {
  const { scrollLines, remainderPx } = dragScrollLines(accumPx, rowHeight)
  // `Math.sign` collapses any multi-row magnitude to -1, 0, or +1 while keeping
  // the drag direction the local path uses.
  return { notch: Math.sign(scrollLines), remainderPx }
}
