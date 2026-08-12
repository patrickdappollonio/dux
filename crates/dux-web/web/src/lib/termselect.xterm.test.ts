// @vitest-environment jsdom
import { Terminal } from "@xterm/xterm"
import { afterEach, describe, expect, it } from "vitest"

import {
  glyphAt,
  rowCells,
  selectionSpan,
  wordRangeAt,
  wordSpanAt,
} from "./termselect"

// Unlike every other terminal test in this repo, these mount the REAL
// `@xterm/xterm`. jsdom cannot back its canvas, so the renderer never measures
// anything and no pixel is painted, but the parser, the buffer and the
// SELECTION MODEL are pure TypeScript and run perfectly well. That is the whole
// point: the questions here (does a length wrap the way `select()` documents?
// is a CJK glyph really two cells? does a programmatic selection break the
// mouse afterwards?) are questions about xterm's real behaviour, and a stub
// would only answer them with whatever we already believed.
//
// Two private fields are forced, and ONLY for the mouse-path test at the
// bottom: `_renderService.dimensions` and `_charSizeService.hasValidSize`, both
// of which come from measuring a canvas jsdom does not implement. Without them
// `MouseService.getCoords` returns undefined for every point and no mouse
// gesture can select anything at all. Nothing under `src/lib` or
// `src/components` reaches into `_core`; this is a test standing in for a
// browser's layout engine.

// xterm's CoreBrowserService tracks the device pixel ratio through
// `matchMedia`, and calls the LEGACY `addListener`, which the shared
// `@/test/matchMedia` stub does not implement (nothing in dux's own code uses
// it). So this file carries its own two-line stand-in rather than widening a
// helper for one caller.
function stubMatchMedia(): () => void {
  const previous = Object.getOwnPropertyDescriptor(window, "matchMedia")
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: (query: string) =>
      ({
        matches: false,
        media: query,
        onchange: null,
        addListener() {},
        removeListener() {},
        addEventListener() {},
        removeEventListener() {},
        dispatchEvent: () => false,
      }) as unknown as MediaQueryList,
  })
  return () => {
    if (previous) Object.defineProperty(window, "matchMedia", previous)
    else delete (window as { matchMedia?: unknown }).matchMedia
  }
}

let restoreMedia: (() => void) | null = null
// Every terminal a test opens, disposed between tests. Not hygiene theatre: a
// live terminal keeps listeners on the shared `document`, and a mouse event
// dispatched there for the NEXT test reaches the previous one as well, which
// made the mouse suite below pass alone and fail in the file.
const opened: Terminal[] = []

afterEach(() => {
  for (const term of opened.splice(0)) term.dispose()
  restoreMedia?.()
  restoreMedia = null
  document.body.innerHTML = ""
})

interface OpenTerminal {
  term: Terminal
  screen: HTMLElement
  /** The cells of one buffer row, exactly as `TerminalPane` reads them. */
  cells: (row: number) => ReturnType<typeof rowCells>
  /** The row accessor `wordSpanAt` takes, built the way the pane builds it. */
  lineAt: (row: number) => { cells: ReturnType<typeof rowCells>; isWrapped: boolean } | undefined
}

async function openTerminal(
  text: string,
  options: { cols?: number; rows?: number } = {},
): Promise<OpenTerminal> {
  restoreMedia = stubMatchMedia()
  const host = document.createElement("div")
  document.body.appendChild(host)
  const term = new Terminal({ cols: options.cols ?? 40, rows: options.rows ?? 10 })
  term.open(host)
  opened.push(term)
  await new Promise<void>((resolve) => term.write(text, resolve))
  const screen = host.querySelector(".xterm-screen") as HTMLElement
  return {
    term,
    screen,
    cells: (row) => rowCells(term.buffer.active.getLine(row)),
    lineAt: (row) => {
      const line = term.buffer.active.getLine(row)
      if (!line) return undefined
      return { cells: rowCells(line), isWrapped: line.isWrapped }
    },
  }
}

/** A single-row word range pinned to one absolute row. */
const anchorOn = (
  word: { startCol: number; endColExclusive: number },
  row: number,
) => ({ startRow: row, endRow: row, ...word })

describe("the pure helpers against a real xterm buffer", () => {
  it("picks the word a long press landed on", async () => {
    const { term, cells } = await openTerminal("git status --porcelain\r\n")
    const word = wordRangeAt(cells(0), 6)
    term.select(word.startCol, 0, word.endColExclusive - word.startCol)
    expect(term.getSelection()).toBe("status")
  })

  it("grows the span forward from the pressed word as the finger drags", async () => {
    const { term, cells } = await openTerminal("git status --porcelain\r\n")
    const word = wordRangeAt(cells(0), 6)
    const span = selectionSpan(anchorOn(word, 0), { col: 13, row: 0 }, term.cols)
    term.select(span.col, span.row, span.length)
    expect(term.getSelection()).toBe("status --p")
  })

  it("normalizes a backwards drag into a selection that still reads forward", async () => {
    const { term, cells } = await openTerminal("git status --porcelain\r\n")
    const word = wordRangeAt(cells(0), 6)
    const span = selectionSpan(anchorOn(word, 0), { col: 0, row: 0 }, term.cols)
    term.select(span.col, span.row, span.length)
    expect(term.getSelection()).toBe("git status")
  })

  it("wraps a multi-row span through the length, which is what makes this work at all", async () => {
    // The claim `select()`'s contract rests on: `finalSelectionEnd` divides the
    // length by `cols`. If a future xterm stopped doing that, every cross-row
    // touch selection would silently truncate to one row and this fails.
    const { term, cells } = await openTerminal("first line\r\nsecond line\r\n", {
      cols: 20,
    })
    const word = wordRangeAt(cells(0), 0)
    const span = selectionSpan(anchorOn(word, 0), { col: 5, row: 1 }, term.cols)
    term.select(span.col, span.row, span.length)
    expect(term.getSelection()).toBe("first line\nsecond")
  })

  it("keeps a CJK word whole from either half of either glyph", async () => {
    const { term, cells } = await openTerminal("日本語 ok\r\n")
    const row = cells(0)
    // The measurement this test exists for: three CJK glyphs, six columns.
    expect(row.slice(0, 6).map((c) => c.width)).toEqual([2, 0, 2, 0, 2, 0])
    for (const col of [0, 1, 2, 3, 4, 5]) {
      const word = wordRangeAt(row, col)
      term.select(word.startCol, 0, word.endColExclusive - word.startCol)
      expect(term.getSelection()).toBe("日本語")
    }
  })

  it("takes an emoji as ONE cell, because that is what xterm makes it", async () => {
    // MEASURED, and it contradicts the obvious assumption, so it is pinned
    // rather than left as folklore: under xterm's DEFAULT Unicode provider (v6
    // wcwidth) an emoji from the U+1F300 block is width ONE, not two. The
    // widths of "🎉😀★→日ａ" come back 1, 1, 1, 1, 2, 2. So the two-cell case
    // this module cares about is CJK and the fullwidth forms; an emoji needs no
    // special handling here at all, and a helper that assumed every emoji was
    // wide would be wrong in this terminal. If dux ever registers the v11
    // provider, these widths move and the CJK tests above are what still hold.
    const { term, cells } = await openTerminal("ship 🎉 it\r\n")
    const row = cells(0)
    expect(row[5]).toEqual({ chars: "🎉", width: 1 })
    const word = wordRangeAt(row, 5)
    term.select(word.startCol, 0, word.endColExclusive - word.startCol)
    expect(term.getSelection()).toBe("🎉")
  })

  it("keeps a fullwidth run whole from either half of any of its cells", async () => {
    const { term, cells } = await openTerminal("ｈｉ ok\r\n")
    const row = cells(0)
    expect(row.slice(0, 4).map((c) => c.width)).toEqual([2, 0, 2, 0])
    for (const col of [0, 1, 2, 3]) {
      const word = wordRangeAt(row, col)
      term.select(word.startCol, 0, word.endColExclusive - word.startCol)
      expect(term.getSelection()).toBe("ｈｉ")
    }
  })

  it("selects a word that begins with a wide glyph and ends in ASCII", async () => {
    const { term, cells } = await openTerminal("日本-cli run\r\n")
    const word = wordRangeAt(cells(0), 5)
    term.select(word.startCol, 0, word.endColExclusive - word.startCol)
    expect(term.getSelection()).toBe("日本-cli")
  })

  it("starts a BACKWARDS drag at the wide glyph, not inside it", async () => {
    // Measured regression: with the anchor on "x" and the finger ending on the
    // CONTINUATION half of 日, the span used to start mid-glyph and the copied
    // text came back as " 本語 x", one glyph short and with a leading blank.
    const { term, cells } = await openTerminal("ok 日本語 x\r\n")
    const row = cells(0)
    const anchor = anchorOn(wordRangeAt(row, 10), 0)
    // Column 4 is the right half of 日, which starts at column 3.
    const focus = glyphAt(row, 4)
    expect(focus).toEqual({ col: 3, width: 2 })
    const span = selectionSpan(anchor, { col: focus.col, row: 0 }, term.cols, focus.width)
    term.select(span.col, span.row, span.length)
    expect(term.getSelection()).toBe("日本語 x")
  })

  it("follows a wrapped path across the physical line break", async () => {
    // A path longer than the terminal is wide: one logical word, two rows,
    // which is the archetypal thing a long press is reaching for.
    const { term, lineAt } = await openTerminal(
      "cd /very/long/path/to/a/file.txt here\r\n",
      { cols: 20 },
    )
    const span = wordSpanAt(lineAt, 0, 8)
    expect(span.startRow).toBe(0)
    expect(span.endRow).toBe(1)
    term.select(
      span.startCol,
      span.startRow,
      (span.endRow - span.startRow) * term.cols +
        span.endColExclusive -
        span.startCol,
    )
    expect(term.getSelection()).toBe("/very/long/path/to/a/file.txt")
  })

  it("stops at the hard break when the next line is not a continuation", async () => {
    const { term, lineAt } = await openTerminal("alpha\r\nbeta\r\n", { cols: 20 })
    expect(wordSpanAt(lineAt, 0, 2)).toEqual({
      startRow: 0,
      startCol: 0,
      endRow: 0,
      endColExclusive: 5,
    })
    expect(term.buffer.active.getLine(1)?.isWrapped).toBe(false)
  })
})

describe("a programmatic selection and the mouse afterwards", () => {
  // The risk this suite exists for: xterm's `setSelection` (what
  // `Terminal.select` calls) runs `_removeMouseDownListeners()`. If that left
  // the pane's DESKTOP mouse selection broken, shipping touch selection would
  // be trading one gesture for another.
  //
  // MEASURED answer, from reading the installed 6.0.0 source through its
  // sourcemap and from the assertions below: `_removeMouseDownListeners` only
  // removes the DOCUMENT-level `mousemove`/`mouseup` pair that
  // `_addMouseDownListeners` installs for the duration of one drag, and clears
  // the drag-scroll interval. The `mousedown` listener on the terminal element
  // is registered once, elsewhere, and is never touched; every
  // `_handleMouseDown` ends by calling `_addMouseDownListeners()` again. So a
  // programmatic selection cancels an IN-FLIGHT drag (correct: the model it was
  // extending just changed under it) and costs the next drag nothing.
  async function openMouseable() {
    const opened = await openTerminal("hello world\r\n", { cols: 20, rows: 5 })
    const rect = (el: HTMLElement) => {
      el.getBoundingClientRect = () =>
        ({
          left: 0,
          top: 0,
          right: 200,
          bottom: 100,
          width: 200,
          height: 100,
          x: 0,
          y: 0,
          toJSON() {},
        }) as DOMRect
    }
    rect(opened.screen)
    const core = (
      opened.term as unknown as {
        _core: { _renderService: object; _charSizeService: object }
      }
    )._core
    Object.defineProperty(core._renderService, "dimensions", {
      configurable: true,
      value: {
        css: { cell: { width: 10, height: 20 }, canvas: { width: 200, height: 100 } },
        device: {
          cell: { width: 10, height: 20 },
          canvas: { width: 200, height: 100 },
          char: { width: 10, height: 20, left: 0, top: 0 },
        },
      },
    })
    Object.defineProperty(core._charSizeService, "hasValidSize", {
      configurable: true,
      value: true,
    })
    return opened
  }

  function drag(screen: HTMLElement, fromX: number, toX: number): void {
    screen.dispatchEvent(
      new MouseEvent("mousedown", {
        bubbles: true,
        clientX: fromX,
        clientY: 0,
        button: 0,
        detail: 1,
      }),
    )
    document.dispatchEvent(
      new MouseEvent("mousemove", {
        bubbles: true,
        clientX: toX,
        clientY: 0,
        button: 0,
        buttons: 1,
      }),
    )
    document.dispatchEvent(
      new MouseEvent("mouseup", { bubbles: true, clientX: toX, clientY: 0, button: 0 }),
    )
  }

  it("still selects with the mouse after a programmatic select", async () => {
    const { term, screen } = await openMouseable()
    term.select(0, 0, 5)
    expect(term.getSelection()).toBe("hello")
    drag(screen, 0, 105)
    expect(term.getSelection()).toBe("hello worl")
  })

  it("re-arms the document drag listeners on the next mousedown", async () => {
    const { term, screen } = await openMouseable()
    term.select(0, 0, 5)
    const added: string[] = []
    const original = document.addEventListener.bind(document)
    document.addEventListener = ((type: string, ...rest: unknown[]) => {
      added.push(type)
      return original(type, ...(rest as [EventListener]))
    }) as typeof document.addEventListener
    try {
      screen.dispatchEvent(
        new MouseEvent("mousedown", {
          bubbles: true,
          clientX: 0,
          clientY: 0,
          button: 0,
          detail: 1,
        }),
      )
    } finally {
      document.addEventListener = original
    }
    expect(added).toContain("mousemove")
    expect(added).toContain("mouseup")
  })

  it("selects with the mouse twice in a row across a programmatic select", async () => {
    const { term, screen } = await openMouseable()
    drag(screen, 0, 45)
    expect(term.getSelection()).toBe("hell")
    term.select(6, 0, 5)
    expect(term.getSelection()).toBe("world")
    drag(screen, 0, 105)
    expect(term.getSelection()).toBe("hello worl")
  })
})
