/**
 * Selecting terminal text with a FINGER.
 *
 * A browser synthesizes mouse events for a TAP and nothing else, so xterm's own
 * selection service, driven entirely by `mousedown`/`mousemove`/`mouseup`, never
 * sees a touch drag. The native browser selection is not an alternative either:
 * xterm's CSS sets `user-select: none` on `.xterm`, and the only descendant that
 * opts back in is the hidden `.xterm-accessibility-tree`. dux therefore drives
 * xterm's OWN selection model through the public
 * `Terminal.select(column, row, length)`, so the highlight, `getSelection()` and
 * the copy path all behave as they do for a mouse.
 *
 * `select()` is a forward start-plus-length and the length WRAPS (in
 * `@xterm/xterm` 6.0.0 `SelectionModel.finalSelectionEnd` divides it by `cols`
 * and adds the quotient to the start row), so an anchor-to-focus span is
 * `(endRow - startRow) * cols + (endCol - startCol)` once the caller has ordered
 * the two ends, which `selectionSpan` does.
 *
 * Everything here is a function over plain data; `TerminalPane` is the thin
 * applicator and the only place that reads real xterm state, through the public
 * `buffer.active.getLine(y)`.
 */

/** A zero-based grid cell. `row` is whatever space the caller is working in. */
export interface Cell {
  col: number
  row: number
}

/** The bounding rect of xterm's `.xterm-screen`, in client coordinates. */
export interface ScreenRect {
  left: number
  top: number
  width: number
  height: number
}

/** The terminal's grid size. */
export interface GridSize {
  cols: number
  rows: number
}

/**
 * The cell under a client point.
 *
 * Measured against the `.xterm-SCREEN` rect, never the pane container: the
 * container includes the scrollbar gutter, so dividing it by the column count
 * inflates every cell and the error accumulates across the row. `.xterm-screen`
 * carries no padding of its own and is sized exactly to the canvas, so
 * `width / cols` is the real cell width.
 *
 * The result is CLAMPED into the grid rather than refused, so a finger that
 * wanders off the edge mid-drag extends the selection to the edge cell, as a
 * mouse drag out of the window does.
 */
export function pointToCell(
  point: { clientX: number; clientY: number },
  rect: ScreenRect,
  grid: GridSize,
): Cell {
  const cellWidth = rect.width / grid.cols
  const cellHeight = rect.height / grid.rows
  const col = Math.floor((point.clientX - rect.left) / cellWidth)
  const row = Math.floor((point.clientY - rect.top) / cellHeight)
  return {
    col: clamp(col, 0, grid.cols - 1),
    row: clamp(row, 0, grid.rows - 1),
  }
}

function clamp(value: number, low: number, high: number): number {
  if (!Number.isFinite(value)) return low
  return Math.min(Math.max(value, low), Math.max(low, high))
}

/**
 * One cell of a buffer row, as the public `IBufferCell` reports it. `width` is 2
 * for a wide glyph, 0 for the CONTINUATION cell after it, 1 otherwise; `chars`
 * is empty for a continuation cell and for a cell never written.
 *
 * Which glyphs are wide is xterm's answer, and it is narrower than "anything
 * that looks big": under its default Unicode v6 provider, CJK and the fullwidth
 * forms are the two-cell case and a U+1F300-block emoji is a single cell.
 */
export interface RowCell {
  chars: string
  width: number
}

/** The slice of xterm's public `IBufferLine` this module reads. */
export interface BufferLineLike {
  length: number
  getCell(x: number): { getChars(): string; getWidth(): number } | undefined
}

/**
 * One buffer row as plain data, the single bridge between xterm and the pure
 * rules below. `getCell` returns `undefined` past the end of the line, which
 * becomes a blank cell rather than a hole, so a row is always exactly `length`
 * cells long and any column index is safe to use.
 */
export function rowCells(line: BufferLineLike | undefined): RowCell[] {
  if (!line) return []
  const cells: RowCell[] = []
  for (let x = 0; x < line.length; x++) {
    const cell = line.getCell(x)
    cells.push({
      chars: cell?.getChars() ?? "",
      width: cell?.getWidth() ?? 1,
    })
  }
  return cells
}

/**
 * The word-separator set: xterm's own default `wordSeparator`, character for
 * character, because a long press and a desktop double-click are the same intent
 * on the same pane and must pick the same word. dux never sets the option, so
 * this is what the mouse path uses.
 */
export const DEFAULT_WORD_SEPARATORS = " ()[]{}',\"`"

/** A half-open column range on one buffer row. */
export interface WordRange {
  startCol: number
  endColExclusive: number
}

/** A resolved glyph: the column it STARTS at, and how many columns it occupies. */
export interface Glyph {
  col: number
  width: number
}

/**
 * The glyph occupying a column.
 *
 * A wide glyph lives in a width-2 cell followed by a width-0 CONTINUATION cell,
 * and a finger on that second cell is on the glyph. Every column reaching the
 * selection arithmetic goes through here in BOTH directions: a forward drag
 * needs the glyph's far edge and a backwards drag its near one, and a backwards
 * drag that skips this step starts the span mid-glyph, dropping the glyph and
 * leaving a stray blank at the front of the copied text.
 *
 * A column past the end of the row answers as itself, one cell wide: the caller
 * is choosing an edge rather than a character.
 */
export function glyphAt(cells: readonly RowCell[], col: number): Glyph {
  if (col < 0 || col >= cells.length) return { col, width: 1 }
  let start = col
  while (start > 0 && cells[start].width === 0) start--
  return { col: start, width: Math.max(cells[start].width, 1) }
}

/**
 * The word occupying `col`, in COLUMNS, on ONE physical row. Working in columns
 * rather than string indexes is what makes wide glyphs fall out for free.
 *
 * Two shapes match xterm's `_getWordAt` on purpose:
 *  - a blank run expands to the whole run, so a press in the gap between two
 *    words selects the gap rather than nothing;
 *  - a NON-blank separator selects only itself, because xterm's expansion
 *    checks the neighbours and never the starting cell.
 *
 * A word that WRAPPED onto the next physical line needs `wordSpanAt`, which
 * composes this one.
 */
export function wordRangeAt(
  cells: readonly RowCell[],
  col: number,
  separators: string = DEFAULT_WORD_SEPARATORS,
): WordRange {
  // A column past the end of the row has no word: an empty range rather than a
  // clamp onto the last cell, so the caller selects nothing instead of
  // something the finger was not on.
  if (col < 0 || col >= cells.length) {
    return { startCol: col, endColExclusive: col }
  }
  let start = glyphAt(cells, col).col

  if (isBlank(cells[start])) {
    let end = start + 1
    while (start > 0 && isBlank(cells[start - 1])) start--
    while (end < cells.length && isBlank(cells[end])) end++
    return { startCol: start, endColExclusive: end }
  }
  let end = start + Math.max(cells[start].width, 1)
  if (isSeparator(cells[start], separators)) {
    return { startCol: start, endColExclusive: end }
  }
  while (start > 0) {
    const prev = glyphAt(cells, start - 1).col
    if (isSeparator(cells[prev], separators)) break
    start = prev
  }
  while (end < cells.length) {
    if (isSeparator(cells[end], separators)) break
    end += Math.max(cells[end].width, 1)
  }
  return { startCol: start, endColExclusive: Math.min(end, cells.length) }
}

function isBlank(cell: RowCell): boolean {
  return cell.chars === "" || cell.chars === " "
}

function isSeparator(cell: RowCell, separators: string): boolean {
  // Never a separator on a continuation cell: it carries no characters, and
  // treating it as one would cut every wide glyph in half.
  if (cell.width === 0) return false
  return isBlank(cell) || separators.includes(cell.chars)
}

/** One physical buffer row, plus whether it CONTINUES the row above it. */
export interface WrappedRow {
  cells: readonly RowCell[]
  /** xterm's public `IBufferLine.isWrapped`. */
  isWrapped: boolean
}

/**
 * The word occupying a cell, FOLLOWED across wrapped lines.
 *
 * A terminal breaks a long line across physical rows marked `isWrapped`, and
 * xterm's double-click follows it, so a long press must too: the two gestures
 * share a separator set precisely to pick the same word, and the archetypal
 * long-press target is a long file path, which is exactly what wraps.
 *
 * The join is decided at the SEAM, one cell either side of the break: the word
 * continues only when the last cell of the upper row and the first of the lower
 * one are both non-separators. A blank run never chases a wrap, because a gap
 * that reaches the edge of the screen is still just a gap.
 */
export function wordSpanAt(
  lineAt: (row: number) => WrappedRow | undefined,
  row: number,
  col: number,
  separators: string = DEFAULT_WORD_SEPARATORS,
): AnchorWord {
  const line = lineAt(row)
  if (!line) {
    return { startRow: row, startCol: col, endRow: row, endColExclusive: col }
  }
  const range = wordRangeAt(line.cells, col, separators)
  const span: AnchorWord = {
    startRow: row,
    startCol: range.startCol,
    endRow: row,
    endColExclusive: range.endColExclusive,
  }
  const empty = range.endColExclusive <= range.startCol
  if (empty || isBlank(line.cells[range.startCol])) return span

  // Upwards: only from column 0 of a row that is itself a continuation.
  for (;;) {
    if (span.startCol !== 0) break
    const here = lineAt(span.startRow)
    if (!here?.isWrapped) break
    const above = lineAt(span.startRow - 1)
    if (!above || above.cells.length === 0) break
    const last = above.cells.length - 1
    if (isSeparator(above.cells[last], separators)) break
    span.startRow -= 1
    span.startCol = wordRangeAt(above.cells, last, separators).startCol
  }
  // Downwards: only into a row that says it continues this one.
  for (;;) {
    const here = lineAt(span.endRow)
    if (!here || span.endColExclusive < here.cells.length) break
    const below = lineAt(span.endRow + 1)
    if (!below?.isWrapped || below.cells.length === 0) break
    if (isSeparator(below.cells[0], separators)) break
    span.endRow += 1
    span.endColExclusive = wordRangeAt(below.cells, 0, separators).endColExclusive
  }
  return span
}

/** The forward triple `Terminal.select(column, row, length)` wants. */
export interface SelectSpan {
  col: number
  row: number
  length: number
}

/**
 * A word pinned to absolute buffer rows. Two rows rather than one, because a
 * word can run over a wrapped line, so its ends are independent positions.
 */
export interface AnchorWord {
  startRow: number
  startCol: number
  endRow: number
  endColExclusive: number
}

/**
 * The span running from the long-pressed WORD out to the finger.
 *
 * The anchor is a word rather than a point because that is the gesture every
 * touch platform ships: a forward drag keeps the word's start and takes the
 * focus cell, a backwards drag keeps the word's end and starts at the focus
 * cell, and a finger still inside the word leaves the whole word selected.
 *
 * Every row here is an ABSOLUTE buffer line
 * (`buffer.active.viewportY + viewportRow`), which is what `select()` takes.
 *
 * `focus` and `focusCellWidth` must come from `glyphAt`, never straight from
 * `pointToCell`: on the right half of a wide glyph the raw column is the
 * CONTINUATION cell, and a backwards drag would start the span inside the glyph.
 * The width applies only on a FORWARD drag, where the focus cell ends the span;
 * on a backwards drag it is the START and its columns are already inside.
 */
export function selectionSpan(
  anchor: AnchorWord,
  focus: Cell,
  cols: number,
  focusCellWidth: number = 1,
): SelectSpan {
  const index = (col: number, row: number) => row * cols + col
  const anchorStart = index(anchor.startCol, anchor.startRow)
  const anchorEnd = index(anchor.endColExclusive, anchor.endRow)
  const focusStart = index(focus.col, focus.row)
  const focusEnd = focusStart + Math.max(focusCellWidth, 1)

  if (focusEnd > anchorEnd) {
    return {
      col: anchor.startCol,
      row: anchor.startRow,
      length: focusEnd - anchorStart,
    }
  }
  if (focusStart < anchorStart) {
    return { col: focus.col, row: focus.row, length: anchorEnd - focusStart }
  }
  return {
    col: anchor.startCol,
    row: anchor.startRow,
    length: anchorEnd - anchorStart,
  }
}

/**
 * How far to scroll so a selection can run past the edge of the viewport.
 *
 * ONE row per move, never a magnitude, for the reason `dragWheelReport` caps a
 * forwarded flick: a touchmove fires at 60-120Hz, so a magnitude would rocket
 * through the scrollback the instant the finger crossed the edge.
 */
export function edgeAutoScroll(clientY: number, rect: ScreenRect): -1 | 0 | 1 {
  if (clientY < rect.top) return -1
  if (clientY > rect.top + rect.height) return 1
  return 0
}
