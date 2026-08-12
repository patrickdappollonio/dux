import { describe, expect, it } from "vitest"

import {
  DEFAULT_WORD_SEPARATORS,
  edgeAutoScroll,
  pointToCell,
  rowCells,
  selectionSpan,
  wordRangeAt,
  type RowCell,
} from "./termselect"

// A 361x480 screen element at (14, 100), 80 columns of 4.5125px and 24 rows of
// 20px. The odd column width is the real shape of the bug this measures against:
// the PANE CONTAINER on the same phone is 374px wide (the scrollbar gutter), so
// a helper that divided the container would put every cell in the wrong place
// by the far side of the row. See the header comment in `lib/termmouse.ts`.
const SCREEN = { left: 14, top: 100, width: 361, height: 480 }
const GRID = { cols: 80, rows: 24 }

const at = (clientX: number, clientY: number) =>
  pointToCell({ clientX, clientY }, SCREEN, GRID)

describe("pointToCell", () => {
  it("puts the origin point on cell 0,0", () => {
    expect(at(SCREEN.left, SCREEN.top)).toEqual({ col: 0, row: 0 })
  })

  it("measures from the screen rect, not from the viewport origin", () => {
    // One cell in and one row down. Anything measuring from 0,0 lands elsewhere.
    expect(at(SCREEN.left + 5, SCREEN.top + 21)).toEqual({ col: 1, row: 1 })
  })

  it("resolves the last cell of the row rather than overflowing it", () => {
    expect(at(SCREEN.left + SCREEN.width - 1, SCREEN.top)).toEqual({
      col: 79,
      row: 0,
    })
  })

  it("clamps a point outside the screen into the grid", () => {
    expect(at(SCREEN.left - 500, SCREEN.top - 500)).toEqual({ col: 0, row: 0 })
    expect(at(SCREEN.left + 5000, SCREEN.top + 5000)).toEqual({
      col: 79,
      row: 23,
    })
  })

  it("agrees with a whole-row sweep computed from the true cell width", () => {
    const cellWidth = SCREEN.width / GRID.cols
    for (let col = 0; col < GRID.cols; col++) {
      const x = SCREEN.left + col * cellWidth + cellWidth / 2
      expect(at(x, SCREEN.top).col).toBe(col)
    }
  })
})

/** Builds a row of single-width cells from an ASCII string. */
function ascii(text: string): RowCell[] {
  return [...text].map((chars) => ({ chars, width: 1 }))
}

/** A wide glyph: the glyph in one cell, then a zero-width continuation cell. */
function wide(chars: string): RowCell[] {
  return [
    { chars, width: 2 },
    { chars: "", width: 0 },
  ]
}

describe("wordRangeAt", () => {
  it("picks the word under the cell", () => {
    const cells = ascii("git status --porcelain")
    expect(wordRangeAt(cells, 5)).toEqual({ startCol: 4, endColExclusive: 10 })
  })

  it("picks the word when the cell is its first or its last", () => {
    const cells = ascii("git status")
    expect(wordRangeAt(cells, 4)).toEqual({ startCol: 4, endColExclusive: 10 })
    expect(wordRangeAt(cells, 9)).toEqual({ startCol: 4, endColExclusive: 10 })
  })

  it("stops at a separator rather than running through it", () => {
    const cells = ascii("run(arg)")
    expect(wordRangeAt(cells, 5)).toEqual({ startCol: 4, endColExclusive: 7 })
  })

  it("selects only the separator when the finger lands on one", () => {
    const cells = ascii("run(arg)")
    expect(wordRangeAt(cells, 3)).toEqual({ startCol: 3, endColExclusive: 4 })
  })

  it("expands a run of spaces, matching a desktop double-click", () => {
    const cells = ascii("a    b")
    expect(wordRangeAt(cells, 3)).toEqual({ startCol: 1, endColExclusive: 5 })
  })

  it("treats an unwritten cell past the end of the text as blank", () => {
    const cells = [...ascii("hi"), ...ascii("  ").map(() => ({ chars: "", width: 1 }))]
    expect(wordRangeAt(cells, 3)).toEqual({ startCol: 2, endColExclusive: 4 })
  })

  it("keeps a whole CJK word, both of whose glyphs are two cells wide", () => {
    // "日本" occupies four columns: 0-1 and 2-3.
    const cells = [...wide("日"), ...wide("本"), ...ascii(" x")]
    expect(wordRangeAt(cells, 0)).toEqual({ startCol: 0, endColExclusive: 4 })
  })

  it("resolves the SECOND cell of a wide glyph to that glyph, not the next one", () => {
    // The trap: column 1 is the continuation half of "日". A helper that read
    // it as its own cell would answer with an empty range, or slide onto "本".
    const cells = [...wide("日"), ...wide("本"), ...ascii(" x")]
    expect(wordRangeAt(cells, 1)).toEqual({ startCol: 0, endColExclusive: 4 })
    expect(wordRangeAt(cells, 3)).toEqual({ startCol: 0, endColExclusive: 4 })
  })

  it("keeps a fullwidth glyph whole from either of its two cells", () => {
    // Deliberately a fullwidth Latin letter rather than an emoji: MEASURED in
    // `termselect.xterm.test.ts`, xterm's default Unicode provider gives an
    // emoji from the U+1F300 block width ONE, so an emoji is not an example of
    // this case in the terminal dux actually ships.
    const cells = [...ascii("hi "), ...wide("ａ"), ...ascii(" ok")]
    expect(wordRangeAt(cells, 3)).toEqual({ startCol: 3, endColExclusive: 5 })
    expect(wordRangeAt(cells, 4)).toEqual({ startCol: 3, endColExclusive: 5 })
  })

  it("runs a wide glyph together with the narrow letters beside it", () => {
    const cells = [...ascii("a"), ...wide("日"), ...ascii("b c")]
    expect(wordRangeAt(cells, 2)).toEqual({ startCol: 0, endColExclusive: 4 })
  })

  it("answers an empty range for a column past the end of the row", () => {
    const cells = ascii("hi")
    expect(wordRangeAt(cells, 9)).toEqual({ startCol: 9, endColExclusive: 9 })
  })

  it("honours a caller-supplied separator set", () => {
    const cells = ascii("a/b c")
    expect(wordRangeAt(cells, 0)).toEqual({ startCol: 0, endColExclusive: 3 })
    expect(wordRangeAt(cells, 0, DEFAULT_WORD_SEPARATORS + "/")).toEqual({
      startCol: 0,
      endColExclusive: 1,
    })
  })
})

describe("rowCells", () => {
  // The one bridge to xterm. `getCell` answers `undefined` past the end of the
  // line, and a hole there would make every column index unsafe downstream.
  const line = {
    length: 4,
    getCell(x: number) {
      const glyphs = ["日", "", "a"]
      if (x >= glyphs.length) return undefined
      return {
        getChars: () => glyphs[x],
        getWidth: () => (x === 0 ? 2 : x === 1 ? 0 : 1),
      }
    },
  }

  it("reads a row into plain cells, one per column", () => {
    expect(rowCells(line)).toEqual([
      { chars: "日", width: 2 },
      { chars: "", width: 0 },
      { chars: "a", width: 1 },
      { chars: "", width: 1 },
    ])
  })

  it("answers an empty row for a line the buffer does not have", () => {
    expect(rowCells(undefined)).toEqual([])
  })
})

const COLS = 80
// The anchor word "status" on absolute buffer row 10, columns 4 through 9.
const ANCHOR = { row: 10, startCol: 4, endColExclusive: 10 }

describe("selectionSpan", () => {
  it("keeps the whole anchor word while the finger has not left it", () => {
    expect(selectionSpan(ANCHOR, { col: 6, row: 10 }, COLS)).toEqual({
      col: 4,
      row: 10,
      length: 6,
    })
  })

  it("extends forward along the row, taking the focus cell itself", () => {
    expect(selectionSpan(ANCHOR, { col: 20, row: 10 }, COLS)).toEqual({
      col: 4,
      row: 10,
      length: 17,
    })
  })

  it("extends forward across rows as a wrapped length", () => {
    expect(selectionSpan(ANCHOR, { col: 2, row: 12 }, COLS)).toEqual({
      col: 4,
      row: 10,
      length: 2 * COLS + 3 - 4,
    })
  })

  it("normalizes a BACKWARDS drag on the same row into a forward span", () => {
    expect(selectionSpan(ANCHOR, { col: 1, row: 10 }, COLS)).toEqual({
      col: 1,
      row: 10,
      length: 9,
    })
  })

  it("normalizes a backwards drag onto an earlier row", () => {
    expect(selectionSpan(ANCHOR, { col: 70, row: 8 }, COLS)).toEqual({
      col: 70,
      row: 8,
      length: 2 * COLS + 10 - 70,
    })
  })

  it("takes both cells of a wide glyph at the forward end", () => {
    expect(selectionSpan(ANCHOR, { col: 20, row: 10 }, COLS, 2)).toEqual({
      col: 4,
      row: 10,
      length: 18,
    })
  })

  it("does not let the focus width leak into a backwards drag", () => {
    expect(selectionSpan(ANCHOR, { col: 1, row: 10 }, COLS, 2)).toEqual({
      col: 1,
      row: 10,
      length: 9,
    })
  })
})

describe("edgeAutoScroll", () => {
  it("does not scroll while the finger is inside the screen", () => {
    expect(edgeAutoScroll(SCREEN.top + 1, SCREEN)).toBe(0)
    expect(edgeAutoScroll(SCREEN.top + SCREEN.height - 1, SCREEN)).toBe(0)
  })

  it("scrolls one row towards older output above the top edge", () => {
    expect(edgeAutoScroll(SCREEN.top - 1, SCREEN)).toBe(-1)
  })

  it("scrolls one row towards newer output below the bottom edge", () => {
    expect(edgeAutoScroll(SCREEN.top + SCREEN.height + 1, SCREEN)).toBe(1)
  })

  it("never returns more than one row, however far past the edge the finger is", () => {
    expect(edgeAutoScroll(SCREEN.top - 4000, SCREEN)).toBe(-1)
    expect(edgeAutoScroll(SCREEN.top + 4000, SCREEN)).toBe(1)
  })
})
