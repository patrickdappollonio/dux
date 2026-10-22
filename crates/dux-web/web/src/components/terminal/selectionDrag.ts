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
//
// While the finger is down the viewport holds still instead of following new
// output, so the text under the finger stays under it; the lift hands the
// viewport back to the output if it was following when the press landed.
import type { IDisposable, IMarker, Terminal } from "@xterm/xterm"

import {
  edgeAutoScroll,
  glyphAt,
  pointToCell,
  rowCells,
  selectionSpan,
  spanHasText,
  wordSpanAt,
  type AnchorWord,
  type ScreenRect,
  type SelectSpan,
} from "@/lib/termselect"

export type SelectionDrag = {
  /// A long press fired: pick the word under the finger and paint it.
  begin: (touch: Touch) => void
  /// The finger moved: grow the span, and start or stop the edge auto-scroll.
  extend: (touch: Touch) => void
  /// The gesture is over (a lift, a cancel, a second finger). The ANCHOR goes;
  /// the painted selection stays.
  end: () => void
  /// Whether the last gesture covered only blank cells, so its lift has
  /// nothing to copy. False for a gesture the terminal cut short (a resize,
  /// the pressed word scrolled out of the buffer, the app switching screens):
  /// there the highlight was taken away, not found empty.
  liftFoundOnlyBlanks: () => boolean
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
  // The pressed word, as the columns it spans plus a MARKER on each row it
  // occupies. A marker belongs to its line, so it moves with that line's text:
  // up when the scrollback at its cap trims lines off the top (which a
  // streaming agent does on every line) or when an app scrolls the whole
  // alternate screen, up or down when an app deletes or inserts lines above
  // it, and it is disposed when its line leaves the buffer, which is the one
  // case where the word is truly gone. Rows captured at press time would
  // slide off the word in every one of those cases. One scroll is NOT
  // followed: a scroll inside a scroll region on the normal screen moves the
  // text without telling any marker, so there the anchor stays on its row.
  //
  // A resize ends the gesture: xterm reflows the buffer and clears its own
  // selection when the rows change, so there is nothing left to extend.
  let anchorCols: { startCol: number; endColExclusive: number } | null = null
  let anchorStart: IMarker | null = null
  let anchorEnd: IMarker | null = null
  // What the last gesture amounted to, read on the lift after `end`.
  let foundOnlyBlanks = false
  // The viewport hold, on the normal screen only (the alternate screen has no
  // scrollback to hold): a marker on the first line on screen, and whether the
  // viewport was following the output when the press landed.
  let holdTop: IMarker | null = null
  let resumeFollowing = false
  let gestureSubscriptions: IDisposable[] = []
  // The finger's last position, so the auto-scroll tick can re-resolve the
  // focus cell without an event of its own.
  let selectPoint: { clientX: number; clientY: number } | null = null
  let selectScrollTimer: ReturnType<typeof setInterval> | undefined
  // Which buffer the markers belong to: an app entering or leaving its alt
  // screen mid-gesture makes them name content that is no longer on screen.
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
  // xterm places a marker relative to the cursor's row; this takes an
  // absolute buffer line.
  const markLine = (row: number): IMarker | undefined => {
    const buffer = term.buffer.active
    return term.registerMarker(row - (buffer.baseY + buffer.cursorY))
  }
  const live = (marker: IMarker | null): marker is IMarker =>
    marker !== null && !marker.isDisposed && marker.line >= 0
  // Puts the viewport back on the line it showed at the press. Runs once a
  // write has been parsed, which is before the frame that would show it is
  // painted, so nothing visibly jumps. Not on xterm's scroll event: that fires
  // before xterm's own viewport has caught up with the line, and a scroll
  // requested there is measured from a stale position. Moving the viewport up
  // is also what tells xterm the user is scrolling, so from then on it keeps
  // the text still by itself and this only has to step in for the first line.
  //
  // An app can delete the very line the hold sits on. The viewport then stays
  // wherever it is: there is no line left to measure from, and the top of the
  // scrollback is the one place the user certainly was not looking.
  const keepViewport = () => {
    if (term.buffer.active.type !== "normal" || !live(holdTop)) return
    if (term.buffer.active.viewportY !== holdTop.line) {
      term.scrollToLine(holdTop.line)
    }
  }
  const holdViewport = () => {
    holdTop?.dispose()
    holdTop = markLine(term.buffer.active.viewportY) ?? null
  }
  const release = () => {
    for (const subscription of gestureSubscriptions) subscription.dispose()
    gestureSubscriptions = []
    anchorStart?.dispose()
    anchorEnd?.dispose()
    anchorStart = null
    anchorEnd = null
    if (holdTop) {
      holdTop.dispose()
      holdTop = null
      // Following resumes on the screen the hold was taken on, and only while
      // it is the one showing: scrolling whatever screen is active now would
      // move a view the user left somewhere on purpose.
      if (resumeFollowing && term.buffer.active.type === "normal") {
        term.scrollToBottom()
      }
    }
    resumeFollowing = false
  }
  // Whether the cells of a span hold any text right now. Cheap: it stops at
  // the first cell with text, and the pressed word is asked first, because a
  // span holding a word that still has text cannot be blank.
  const hasText = (span: SelectSpan, anchor: AnchorWord): boolean => {
    const rowAt = (row: number) => lineAt(row)?.cells
    const word = {
      col: anchor.startCol,
      row: anchor.startRow,
      length:
        (anchor.endRow - anchor.startRow) * term.cols +
        anchor.endColExclusive -
        anchor.startCol,
    }
    return spanHasText(rowAt, word, term.cols) || spanHasText(rowAt, span, term.cols)
  }
  // The verdict the lift reads, settled when the gesture ends. A gesture the
  // terminal cut short (a resize, the app switching screens, the pressed word
  // leaving the buffer) was never found blank, whatever is painted. Otherwise
  // the painted cells are asked as they are NOW: the app may have redrawn
  // them as spaces under a still finger, and a highlight over blank cells
  // promises a copy that would be empty.
  const settle = (cutShort: boolean) => {
    if (!anchorCols) return
    const anchor = currentAnchor()
    if (cutShort || term.buffer.active.type !== selectBuffer || !anchor) {
      foundOnlyBlanks = false
      return
    }
    if (foundOnlyBlanks) return
    const painted = term.getSelectionPosition()
    if (!painted) return
    const span = {
      col: painted.start.x,
      row: painted.start.y,
      length:
        (painted.end.y - painted.start.y) * term.cols + painted.end.x - painted.start.x,
    }
    if (!hasText(span, anchor)) select(span.col, span.row, span.length, true)
  }
  const finish = (cutShort: boolean) => {
    settle(cutShort)
    stopSelectAutoScroll()
    release()
    // The ANCHOR is per gesture; the SELECTION deliberately outlives it, so
    // the highlight stays on screen after the copy until the next tap.
    anchorCols = null
    selectPoint = null
    selectBuffer = ""
  }
  const end = () => finish(false)
  // The terminal took the gesture away: whatever is painted is what the user
  // last saw, and it was not found blank.
  const interrupt = () => finish(true)
  const select = (col: number, row: number, length: number, blank: boolean) => {
    foundOnlyBlanks = blank
    // A highlight over blank cells promises a copy that would be empty.
    if (blank) term.clearSelection()
    else term.select(col, row, length)
  }
  const begin = (touch: Touch): void => {
    foundOnlyBlanks = false
    const rect = screenRect()
    if (!rect) return
    // A long press starts a new selection, so the previous highlight goes even
    // when this one lands on nothing.
    term.clearSelection()
    const cell = pointToCell(touch, rect, grid())
    const span = wordSpanAt(lineAt, absoluteRow(cell.row), cell.col)
    const length =
      (span.endRow - span.startRow) * term.cols +
      span.endColExclusive -
      span.startCol
    foundOnlyBlanks = true
    if (length <= 0) return
    const start = markLine(span.startRow) ?? null
    const endMarker =
      span.endRow === span.startRow ? start : (markLine(span.endRow) ?? null)
    if (!live(start) || !live(endMarker)) {
      start?.dispose()
      endMarker?.dispose()
      return
    }
    anchorStart = start
    anchorEnd = endMarker === start ? null : endMarker
    anchorCols = { startCol: span.startCol, endColExclusive: span.endColExclusive }
    selectBuffer = term.buffer.active.type
    selectPoint = { clientX: touch.clientX, clientY: touch.clientY }
    gestureSubscriptions.push(term.onResize(interrupt))
    if (selectBuffer === "normal") {
      const buffer = term.buffer.active
      resumeFollowing = buffer.viewportY >= buffer.baseY
      holdViewport()
      gestureSubscriptions.push(term.onWriteParsed(keepViewport))
    }
    const pressed = { col: span.startCol, row: span.startRow, length }
    select(pressed.col, pressed.row, length, !hasText(pressed, span))
    // A short buzz is the platform's own "you are now selecting" signal. Guarded
    // twice: Safari implements no Vibration API, and one that does may throw
    // without user activation.
    try {
      navigator.vibrate?.(10)
    } catch {
      // A missing buzz is not worth failing a selection over.
    }
  }
  // The pressed word where its text is NOW, or null once its line has left
  // the buffer.
  const currentAnchor = (): AnchorWord | null => {
    const cols = anchorCols
    const endMarker = anchorEnd ?? anchorStart
    if (!cols || !live(anchorStart) || !live(endMarker)) return null
    return {
      startRow: anchorStart.line,
      startCol: cols.startCol,
      endRow: endMarker.line,
      endColExclusive: cols.endColExclusive,
    }
  }
  // Re-selects from the anchor to wherever `selectPoint` is. It reads the stored
  // point rather than taking one because the auto-scroll tick calls it too.
  const apply = (): void => {
    const point = selectPoint
    if (!anchorCols || !point) return
    const anchor = currentAnchor()
    if (term.buffer.active.type !== selectBuffer || !anchor) {
      // The app swapped screens under the gesture, or the pressed word left
      // the buffer, so the anchor describes nothing on screen. The painted
      // selection stays: it is what the user last saw.
      interrupt()
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
    // Asked every move rather than once at the press: a marker follows its
    // line, not its text, and an app redrawing that line (a spinner, a status
    // bar) can turn the pressed word into spaces.
    select(span.col, span.row, span.length, !hasText(span, anchor))
  }
  const autoScrollTick = (): void => {
    const point = selectPoint
    const rect = screenRect()
    if (!point || !rect || !anchorCols) {
      stopSelectAutoScroll()
      return
    }
    const direction = edgeAutoScroll(point.clientY, rect)
    if (direction === 0) {
      stopSelectAutoScroll()
      return
    }
    // One row per TICK. Deliberately not a magnitude: the point is a readable
    // walk the user can stop by moving back inside, not a jump. The hold moves
    // with it, so the viewport stays where the walk left it.
    term.scrollLines(direction)
    if (holdTop) holdViewport()
    apply()
  }
  const extend = (touch: Touch): void => {
    selectPoint = { clientX: touch.clientX, clientY: touch.clientY }
    const rect = screenRect()
    const past = rect ? edgeAutoScroll(touch.clientY, rect) !== 0 : false
    if (past && anchorCols) {
      if (selectScrollTimer === undefined) {
        selectScrollTimer = setInterval(autoScrollTick, SELECT_SCROLL_INTERVAL_MS)
      }
    } else {
      stopSelectAutoScroll()
    }
    apply()
  }
  return {
    begin,
    extend,
    end,
    liftFoundOnlyBlanks: () => foundOnlyBlanks,
    active: () => anchorCols !== null,
  }
}
