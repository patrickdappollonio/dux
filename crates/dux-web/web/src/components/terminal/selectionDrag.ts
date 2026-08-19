// THE SELECTION-DRAG MACHINE.
//
// A browser synthesizes mouse events for a TAP and for nothing else, so xterm's
// own selection service (driven entirely by mousedown/mousemove/mouseup) has
// never seen a touch drag and has never produced a selection from one. Nor can
// the BROWSER select the output: xterm.css puts `user-select: none` on `.xterm`
// itself. So dux drives xterm's own selection model through the public
// `Terminal.select`, and the arithmetic and the word rules live in the pure
// `lib/termselect.ts`.
//
// The gesture is the one every touch platform ships: a long press picks the
// WORD under the finger, a drag grows the span from whichever end of that word
// the finger has passed, a drag past an edge auto-scrolls, and the lift copies
// (which the lifecycle does, through the same preference the mouse path uses).
//
// It is FED BY THE GESTURE MACHINE and owns nothing about disambiguation: it is
// told to begin, to extend, and to end. Its highlight deliberately OUTLIVES the
// gesture, because the highlight is the result the user asked for; only the
// ANCHOR is per gesture.
//
// It ALWAYS selects locally, even when the app in the PTY has mouse tracking
// on, which makes it the touch equivalent of the desktop force-local-selection
// modifier (Shift on Linux/Windows, Option on macOS). Claude Code and opencode
// both take the mouse, so a long press that forwarded instead would leave every
// real agent pane unselectable by finger.
import type { Terminal } from "@xterm/xterm"

import {
  edgeAutoScroll,
  glyphAt,
  pointToCell,
  rowCells,
  selectionSpan,
  wordSpanAt,
  type AnchorWord,
  type ScreenRect,
} from "@/lib/termselect"

export type SelectionDrag = {
  /// A long press fired: pick the word under the finger and paint it.
  begin: (touch: Touch) => void
  /// The finger moved: grow the span, and start or stop the edge auto-scroll.
  extend: (touch: Touch) => void
  /// The gesture is over (a lift, a cancel, a second finger). The ANCHOR goes;
  /// the painted selection stays.
  end: () => void
  /// Whether a selection gesture is currently anchored.
  /// TEST-ONLY accessor: no production client reads it; the unit tests use it
  /// to assert the highlight-outlives-gesture semantics without poking internals.
  active: () => boolean
}

export function createSelectionDrag(term: Terminal): SelectionDrag {
  // How often the viewport walks while the finger is parked past an edge.
  // A TIMER, not one row per touchmove: a finger held still at the edge
  // produces no further events, so an event-driven version stopped dead and
  // the user had to jiggle to keep extending. xterm's own mouse drag scroll
  // is a 50ms interval for exactly this reason (`DRAG_SCROLL_INTERVAL`).
  const SELECT_SCROLL_INTERVAL_MS = 50
  // ACCEPTED, LIKE THE TRIM LIMIT BELOW: a selection drag never holds the
// fit/SIGWINCH pair (the resize hold is scroll-scoped), so a debounced resize
// can in principle land mid-selection and shift the anchor's rows. The driving
// keyboard-collapse scenario cannot occur here (a selection never blurs the
// typing surface), and the cost class is the same "wrong selection, lift and
// retry".
// KNOWN LIMIT, assessed and deliberately not guarded. `selectAnchor` holds
  // ABSOLUTE buffer rows captured at press time. When the scrollback ring is
  // already full and the child writes more output, xterm TRIMS lines off the
  // top, every absolute row shifts, and the anchor then names different
  // content for the rest of the gesture. xterm compensates its own model from
  // `lines.onTrim`, which is INTERNAL: the public surface (`IBuffer`,
  // `Terminal.onLineFeed`, `buffer.onBufferChange`) publishes no trim signal,
  // and no combination of `length`/`baseY` distinguishes "scrolled" from
  // "trimmed" once the ring is at its cap. Inferring one from `onLineFeed`
  // would miss every scroll that is not a linefeed (IND, `CSI S`), and
  // snapshotting the anchor row's text would fire on any in-place repaint. So
  // there is no cheap CORRECT guard here, and a fragile one is worse than the
  // bug: it needs a busy agent writing during the second or two a drag lasts,
  // it costs the user a wrong selection and nothing else, and lifting and
  // pressing again fixes it.
  let selectAnchor: AnchorWord | null = null
  // The finger's last position, so the auto-scroll tick can re-resolve the
  // focus cell without an event of its own.
  let selectPoint: { clientX: number; clientY: number } | null = null
  let selectScrollTimer: ReturnType<typeof setInterval> | undefined
  // Which buffer the anchor's rows belong to. An app entering or leaving its
  // alt screen mid-gesture invalidates every one of them: a normal-buffer row
  // number applied to the alt buffer names unrelated content. Abandoning is
  // the only honest answer.
  let selectBuffer = ""
  const stopSelectAutoScroll = () => {
    clearInterval(selectScrollTimer)
    selectScrollTimer = undefined
  }
  // xterm's `.xterm-screen`, which is what the cell math must measure: the
  // pane CONTAINER is wider by the scrollbar gutter, and dividing that by the
  // column count drifts two columns by the far side of the row (MEASURED; see
  // `lib/termmouse.ts`). A zero-sized rect means the terminal is not laid out
  // yet, and there is no cell to answer with.
  const screenRect = (): ScreenRect | null => {
    const screen = term.element?.querySelector(".xterm-screen")
    if (!screen) return null
    const r = screen.getBoundingClientRect()
    if (!r.width || !r.height) return null
    return { left: r.left, top: r.top, width: r.width, height: r.height }
  }
  const grid = () => ({ cols: term.cols, rows: term.rows })
  // A viewport row is only meaningful for the frame it was measured in;
  // `select()` takes an ABSOLUTE buffer line, so every row crosses through
  // `viewportY` here and nowhere else.
  const absoluteRow = (viewportRow: number) =>
    term.buffer.active.viewportY + viewportRow
  // The row accessor `wordSpanAt` walks, so a word that wrapped onto the next
  // physical line is picked whole (`isWrapped` is public API).
  const lineAt = (row: number) => {
    const line = term.buffer.active.getLine(row)
    if (!line) return undefined
    return { cells: rowCells(line), isWrapped: line.isWrapped }
  }
  const end = () => {
    stopSelectAutoScroll()
    // The ANCHOR is per gesture; the SELECTION deliberately outlives it, so
    // the highlight stays on screen after the copy until the next tap.
    selectAnchor = null
    selectPoint = null
    selectBuffer = ""
  }
  const begin = (touch: Touch): void => {
    const rect = screenRect()
    if (!rect) return
    const cell = pointToCell(touch, rect, grid())
    const span = wordSpanAt(lineAt, absoluteRow(cell.row), cell.col)
    const length =
      (span.endRow - span.startRow) * term.cols +
      span.endColExclusive -
      span.startCol
    if (length <= 0) return
    selectAnchor = span
    selectBuffer = term.buffer.active.type
    selectPoint = { clientX: touch.clientX, clientY: touch.clientY }
    term.select(span.startCol, span.startRow, length)
    // A short buzz is the platform's own "you are now selecting" signal.
    // Guarded twice over: Safari implements no Vibration API at all, and a
    // browser that does may still throw when the page lacks user activation.
    try {
      navigator.vibrate?.(10)
    } catch {
      // A missing buzz is not worth failing a selection over.
    }
  }
  // Re-selects from the anchor to wherever `selectPoint` currently is. Called
  // both from a touchmove and from the auto-scroll tick, which is why it
  // reads the stored point rather than taking one.
  const apply = (): void => {
    const anchor = selectAnchor
    const point = selectPoint
    if (!anchor || !point) return
    if (term.buffer.active.type !== selectBuffer) {
      // The app swapped buffers under the gesture. Abandon rather than
      // applying the anchor's rows to a buffer they do not describe; the
      // painted selection is left alone, since it is what the user last saw.
      end()
      return
    }
    const rect = screenRect()
    if (!rect) return
    const cell = pointToCell(point, rect, grid())
    const row = absoluteRow(cell.row)
    const cells = rowCells(term.buffer.active.getLine(row))
    // Resolve the column to the GLYPH that owns it before any arithmetic: on
    // the right half of a wide glyph the raw column is a continuation cell,
    // and a backwards drag would then start the span inside the glyph.
    const focus = glyphAt(cells, cell.col)
    const span = selectionSpan(anchor, { col: focus.col, row }, term.cols, focus.width)
    term.select(span.col, span.row, span.length)
  }
  const autoScrollTick = (): void => {
    const point = selectPoint
    const rect = screenRect()
    if (!point || !rect || !selectAnchor) {
      stopSelectAutoScroll()
      return
    }
    const direction = edgeAutoScroll(point.clientY, rect)
    if (direction === 0) {
      stopSelectAutoScroll()
      return
    }
    // One row per TICK. Deliberately not a magnitude: the point is a readable
    // walk the user can stop by moving back inside, not a jump.
    term.scrollLines(direction)
    apply()
  }
  const extend = (touch: Touch): void => {
    selectPoint = { clientX: touch.clientX, clientY: touch.clientY }
    const rect = screenRect()
    const past = rect ? edgeAutoScroll(touch.clientY, rect) !== 0 : false
    if (past && selectAnchor) {
      if (selectScrollTimer === undefined) {
        selectScrollTimer = setInterval(autoScrollTick, SELECT_SCROLL_INTERVAL_MS)
      }
    } else {
      stopSelectAutoScroll()
    }
    apply()
  }
  return { begin, extend, end, active: () => selectAnchor !== null }
}
