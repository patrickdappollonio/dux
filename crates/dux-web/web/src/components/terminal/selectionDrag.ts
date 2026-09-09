// A browser synthesizes mouse events for a tap and nothing else, so xterm's own
// selection service has never seen a touch drag, and the browser cannot select
// the output either (xterm.css puts `user-select: none` on `.xterm`). So dux
// drives xterm's selection model through the public `Terminal.select`, with the
// arithmetic and word rules in the pure `lib/termselect.ts`.
//
// The gesture is the one every touch platform ships: a long press picks the word
// under the finger, a drag grows the span from whichever end the finger passed,
// a drag past an edge auto-scrolls, and the lift copies through the same
// preference the mouse path uses.
//
// The gesture machine feeds it and owns disambiguation; this is told to begin,
// extend and end. Its highlight outlives the gesture, because the highlight is
// the result the user asked for; only the anchor is per gesture.
//
// It always selects locally, even under mouse tracking, which makes it the touch
// equivalent of the desktop force-local-selection modifier. Agent CLIs routinely
// take the mouse, so forwarding instead leaves their panes unselectable.
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
  /// Whether a selection gesture is currently anchored. No production client
  /// reads it; it exists so the highlight-outlives-gesture semantics are
  /// assertable without poking internals.
  active: () => boolean
}

export function createSelectionDrag(term: Terminal): SelectionDrag {
  // How often the viewport walks while the finger is parked past an edge. A
  // timer, not one row per touchmove: a finger held still produces no further
  // events. xterm's own drag scroll is an interval for the same reason.
  const SELECT_SCROLL_INTERVAL_MS = 50
  // The anchor holds absolute buffer rows captured at press time, and two things
  // can shift those mid-gesture: a debounced resize (a selection drag takes no
  // resize hold, which is scroll-scoped), and xterm trimming lines off the top
  // once the scrollback ring is at its cap.
  //
  // Neither is guarded. xterm compensates its own model from the internal
  // `lines.onTrim`; the public surface publishes no trim signal, and no
  // combination of `length` and `baseY` tells a scroll from a trim at the cap.
  // Both cost one wrong selection that a lift and a fresh press fix.
  let selectAnchor: AnchorWord | null = null
  // The finger's last position, so the auto-scroll tick can re-resolve the
  // focus cell without an event of its own.
  let selectPoint: { clientX: number; clientY: number } | null = null
  let selectScrollTimer: ReturnType<typeof setInterval> | undefined
  // Which buffer the anchor's rows belong to: an app entering or leaving its alt
  // screen mid-gesture makes every one of them name unrelated content.
  let selectBuffer = ""
  const stopSelectAutoScroll = () => {
    clearInterval(selectScrollTimer)
    selectScrollTimer = undefined
  }
  // The cell math must measure xterm's `.xterm-screen`: the pane container is
  // wider by the scrollbar gutter, and dividing that by the column count drifts
  // columns by the far side of the row. A zero-sized rect means no layout yet.
  const screenRect = (): ScreenRect | null => {
    const screen = term.element?.querySelector(".xterm-screen")
    if (!screen) return null
    const r = screen.getBoundingClientRect()
    if (!r.width || !r.height) return null
    return { left: r.left, top: r.top, width: r.width, height: r.height }
  }
  const grid = () => ({ cols: term.cols, rows: term.rows })
  // A viewport row means something only for the frame it was measured in, and
  // `select()` takes an absolute buffer line, so rows cross over here alone.
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
    // A short buzz is the platform's own "you are now selecting" signal. Guarded
    // twice: Safari implements no Vibration API, and one that does may throw
    // without user activation.
    try {
      navigator.vibrate?.(10)
    } catch {
      // A missing buzz is not worth failing a selection over.
    }
  }
  // Re-selects from the anchor to wherever `selectPoint` is. It reads the stored
  // point rather than taking one because the auto-scroll tick calls it too.
  const apply = (): void => {
    const anchor = selectAnchor
    const point = selectPoint
    if (!anchor || !point) return
    if (term.buffer.active.type !== selectBuffer) {
      // The app swapped buffers under the gesture, so the anchor's rows describe
      // nothing here. The painted selection stays: it is what the user last saw.
      end()
      return
    }
    const rect = screenRect()
    if (!rect) return
    const cell = pointToCell(point, rect, grid())
    const row = absoluteRow(cell.row)
    const cells = rowCells(term.buffer.active.getLine(row))
    // Resolve the column to the glyph that owns it before any arithmetic: the
    // right half of a wide glyph is a continuation cell, and a backwards drag
    // would start the span inside the glyph.
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
