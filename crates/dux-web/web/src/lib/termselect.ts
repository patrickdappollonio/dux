/**
 * Selecting terminal text with a FINGER.
 *
 * # Why there is anything to write at all
 *
 * A browser synthesizes mouse events for a TAP and for nothing else, so xterm's
 * own selection service (which is driven entirely by `mousedown` /`mousemove` /
 * `mouseup`) never sees a touch drag and has never produced a selection from
 * one. The native browser selection is not an alternative either:
 * `node_modules/@xterm/xterm/css/xterm.css` sets `user-select: none` on `.xterm`
 * itself, and the only descendant that opts back in is the (hidden)
 * `.xterm-accessibility-tree`, so the browser is forbidden from selecting the
 * output whatever the gesture. That CSS is deliberate and is left alone here:
 * dux drives xterm's OWN selection model instead, through the public
 * `Terminal.select(column, row, length)`, so the highlight, `getSelection()`
 * and the existing copy path all keep working exactly as they do for a mouse.
 *
 * `select()` is a forward start-plus-length, and the length WRAPS: MEASURED in
 * the installed `@xterm/xterm` 6.0.0, `SelectionModel.finalSelectionEnd`
 * divides `selectionStartLength` by `cols` and adds the quotient to the start
 * row. So an arbitrary anchor-to-focus span is expressible as
 * `(endRow - startRow) * cols + (endCol - startCol)`, provided the caller has
 * ordered the two ends first. That ordering is `selectionSpan` below.
 *
 * # Everything here is pure
 *
 * Same house style as `lib/termmouse.ts` and `lib/termkeys.ts`: the arithmetic
 * and the word rules are functions over plain data, unit-tested without
 * mounting xterm, and `TerminalPane` is the thin applicator. The one place that
 * reads real xterm state is the pane, which lifts a row out of
 * `buffer.active.getLine(y)` (all public API; nothing here touches
 * `term._core`).
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
 * Measured against the `.xterm-SCREEN` rect, never the pane container. This is
 * the exact error `lib/termmouse.ts` documents at length: MEASURED on a 390px
 * phone the container is 374px wide where the screen element is 361px (the
 * scrollbar gutter), so dividing the container by the column count inflates
 * every cell and the error accumulates across the row, drifting two columns by
 * the far side. `.xterm-screen` carries no padding of its own (checked in
 * `xterm.css`) and is sized exactly to the canvas, so `width / cols` is the
 * real cell width.
 *
 * The result is CLAMPED into the grid rather than refused: a finger that has
 * wandered off the edge mid-drag should extend the selection to the edge cell,
 * which is what a mouse drag out of the window does too.
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
 * One cell of a buffer row, as the public `IBufferCell` reports it.
 *
 * `width` is 2 for a wide glyph, 0 for the CONTINUATION cell that follows it,
 * and 1 otherwise. `chars` is empty for a continuation cell and for a cell that
 * was never written.
 *
 * Which glyphs are wide is xterm's answer, not this module's, and it is worth
 * knowing that it is narrower than "anything that looks big": MEASURED against
 * the installed 6.0.0 with its DEFAULT (Unicode v6) provider, the widths of
 * `🎉😀★→日ａ` are 1, 1, 1, 1, 2, 2. So CJK and the fullwidth forms are the
 * two-cell case, and an emoji from the U+1F300 block is a single cell here.
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
 * One buffer row as plain data.
 *
 * The single bridge between xterm and the pure rules below, so a test can build
 * a row by hand and the pane can hand over a real one. `getCell` returns
 * `undefined` past the end of the line, which becomes a blank cell rather than
 * a hole, so a row is always exactly `length` cells long and a column index is
 * always safe to use.
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
 * The word-separator set.
 *
 * Deliberately xterm's own default `wordSeparator` option, character for
 * character (MEASURED in the installed 6.0.0 bundle's option defaults). The
 * reason is not that this set is objectively right, it is that a long press and
 * a desktop double-click are the SAME user intent on the same pane, so they
 * must pick the same word. dux never sets the option, so this is what the mouse
 * path uses.
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
 * A wide glyph (CJK, the fullwidth forms) lives in a width-2 cell followed by a
 * width-0 CONTINUATION cell, and a finger landing on that second cell is
 * landing on the glyph, not between two of them. Every column that reaches the
 * selection arithmetic goes through here first, in BOTH directions: a forward
 * drag needs the glyph's far edge and a backwards drag needs its near one, and
 * a backwards drag that skipped this step started the span mid-glyph, dropping
 * the glyph and leaving a stray blank at the front of the copied text.
 *
 * A column past the end of the row answers as itself, one cell wide: there is
 * no glyph there, and the caller is choosing an edge rather than a character.
 */
export function glyphAt(cells: readonly RowCell[], col: number): Glyph {
  if (col < 0 || col >= cells.length) return { col, width: 1 }
  let start = col
  while (start > 0 && cells[start].width === 0) start--
  return { col: start, width: Math.max(cells[start].width, 1) }
}

/**
 * The word occupying `col`, in COLUMNS, on ONE physical row.
 *
 * Working in columns rather than in string indexes is what makes wide glyphs
 * fall out for free, through `glyphAt` above.
 *
 * Two shapes match xterm's `_getWordAt` on purpose:
 *  - a blank run expands to the whole run, so a press in the gap between two
 *    words selects the gap rather than nothing;
 *  - a NON-blank separator selects only itself, because xterm's expansion
 *    checks the neighbours and never the starting cell.
 *
 * This is the single-row primitive. A word that WRAPPED onto the next physical
 * line needs `wordSpanAt`, which composes this one.
 */
export function wordRangeAt(
  cells: readonly RowCell[],
  col: number,
  separators: string = DEFAULT_WORD_SEPARATORS,
): WordRange {
  // A column past the end of the row has no word. Answer an empty range there
  // rather than clamping onto the last cell, so the caller selects nothing
  // instead of something the finger was not on.
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
 * A terminal breaks a long line across physical rows and marks each
 * continuation `isWrapped`; the text is one logical line, and xterm's own
 * double-click follows it. A long press has to as well, for two reasons: the
 * separator set here is xterm's precisely so that the two gestures pick the
 * same word, and the archetypal thing a user reaches for a long press to grab
 * is one long file path, which is exactly the thing that wraps.
 *
 * The join is decided at the SEAM, one cell either side of the break: the word
 * continues only when the last cell of the upper row and the first cell of the
 * lower one are both non-separators. A blank run never chases a wrap, because a
 * gap that happens to reach the edge of the screen is still just a gap.
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
 * A word pinned to absolute buffer rows.
 *
 * Two rows rather than one, because a word can run over a wrapped line: the
 * start and the end are independent positions, and `wordSpanAt` is what finds
 * them.
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
 * touch platform ships: the press picks a word and the drag grows the selection
 * from whichever END of it the finger is past. So a forward drag keeps the
 * word's start and takes the focus cell, a backwards drag keeps the word's end
 * and starts at the focus cell, and a finger still inside the word leaves the
 * whole word selected.
 *
 * Every row here is an ABSOLUTE buffer line
 * (`buffer.active.viewportY + viewportRow`), which is what `select()` takes.
 *
 * `focus` and `focusCellWidth` must come from `glyphAt`, never straight from
 * `pointToCell`: on the right half of a wide glyph the raw column is the
 * CONTINUATION cell, and a backwards drag would then start the span inside the
 * glyph. The width is applied only on a FORWARD drag, where the focus cell is
 * the end of the span; on a backwards drag the focus cell is the START and its
 * own columns are already inside it.
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
 * forwarded flick: a touchmove fires at 60-120Hz, so a magnitude here would
 * rocket through the scrollback the instant the finger crossed the edge. One
 * row per event tracks the finger at a readable speed and stops the moment it
 * comes back inside.
 */
export function edgeAutoScroll(clientY: number, rect: ScreenRect): -1 | 0 | 1 {
  if (clientY < rect.top) return -1
  if (clientY > rect.top + rect.height) return 1
  return 0
}
