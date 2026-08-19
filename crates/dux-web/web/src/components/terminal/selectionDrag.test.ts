// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Terminal } from "@xterm/xterm"

import { createSelectionDrag } from "./selectionDrag"

// The cell arithmetic and the word rules are pinned against a REAL xterm buffer
// in `lib/termselect.xterm.test.ts`; what these tests pin is the MACHINE around
// them: when an anchor is taken, when it is abandoned, when the auto-scroll
// timer runs, and what survives the end of a gesture.
const CELL_W = 10
const CELL_H = 20
const LEFT = 100
const TOP = 50

class TermFake {
  cols = 20
  rows = 4
  lines = ["git status", "second line"]
  selection: { col: number; row: number; length: number } | null = null
  scrolled: number[] = []
  buffer = {
    active: {
      type: "normal",
      viewportY: 0,
      getLine: (y: number) => {
        const text = this.lines[y]
        if (text === undefined) return undefined
        const chars = [...text]
        return {
          length: this.cols,
          isWrapped: false,
          getCell: (x: number) => ({
            getChars: () => chars[x] ?? "",
            getWidth: () => 1,
          }),
        }
      },
    },
  }
  element: HTMLElement
  constructor() {
    const el = document.createElement("div")
    const screen = document.createElement("div")
    screen.className = "xterm-screen"
    el.appendChild(screen)
    screen.getBoundingClientRect = () =>
      ({
        left: LEFT,
        top: TOP,
        width: this.cols * CELL_W,
        height: this.rows * CELL_H,
        right: LEFT + this.cols * CELL_W,
        bottom: TOP + this.rows * CELL_H,
        x: LEFT,
        y: TOP,
        toJSON() {},
      }) as DOMRect
    this.element = el
  }
  select(col: number, row: number, length: number) {
    this.selection = { col, row, length }
  }
  scrollLines(n: number) {
    this.scrolled.push(n)
    this.buffer.active.viewportY = Math.max(0, this.buffer.active.viewportY + n)
  }
}

/// A touch at the given cell's centre.
function at(col: number, row: number): Touch {
  return {
    clientX: LEFT + col * CELL_W + CELL_W / 2,
    clientY: TOP + row * CELL_H + CELL_H / 2,
  } as Touch
}

function setup() {
  const term = new TermFake()
  const drag = createSelectionDrag(term as unknown as Terminal)
  return { term, drag }
}

beforeEach(() => {
  vi.useFakeTimers()
})
afterEach(() => {
  vi.useRealTimers()
})

describe("beginning a selection", () => {
  it("selects the word under the finger and takes an anchor", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 0))
    expect(term.selection).toEqual({ col: 0, row: 0, length: 3 })
    expect(drag.active()).toBe(true)
  })

  it("selects only blanks where there is nothing, rather than the nearest word", () => {
    const { term, drag } = setup()
    // Past the end of the text: a blank run, which expands as a run (xterm's
    // own word rule) rather than reaching back for the word to its left.
    drag.begin(at(15, 0))
    expect(term.selection?.col).toBeGreaterThanOrEqual(10)
  })

  it("takes no anchor at all on a row that does not exist", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 3))
    expect(term.selection).toBeNull()
    expect(drag.active()).toBe(false)
  })

  it("answers nothing at all when the terminal is not laid out yet", () => {
    const { term, drag } = setup()
    const screen = term.element.querySelector(".xterm-screen") as HTMLElement
    screen.getBoundingClientRect = () => ({ width: 0, height: 0 }) as DOMRect
    drag.begin(at(1, 0))
    expect(term.selection).toBeNull()
    expect(drag.active()).toBe(false)
  })
})

describe("extending a selection", () => {
  it("grows the span to the finger's cell", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 0))
    drag.extend(at(8, 0))
    expect(term.selection).toEqual({ col: 0, row: 0, length: 9 })
  })

  it("carries the selection onto the next row", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 0))
    drag.extend(at(3, 1))
    // The span wraps through the LENGTH, at the column count, which is what
    // makes xterm's forward start-plus-length model work across rows.
    expect(term.selection).toEqual({ col: 0, row: 0, length: 20 + 4 })
  })

  it("does nothing when no long press ever anchored the gesture", () => {
    const { term, drag } = setup()
    drag.extend(at(5, 1))
    expect(term.selection).toBeNull()
  })

  it("abandons the gesture when the app flips buffers mid-drag, leaving the paint alone", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 0))
    const painted = term.selection
    term.buffer.active.type = "alternate"
    drag.extend(at(8, 0))
    expect(drag.active()).toBe(false)
    // The highlight the user last saw is untouched: a normal-buffer row number
    // applied to the alt buffer names unrelated content, so abandoning is the
    // only honest answer.
    expect(term.selection).toEqual(painted)
  })
})

describe("the edge auto-scroll", () => {
  it("keeps walking while the finger is parked past the bottom edge", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 0))
    drag.extend({ clientX: LEFT + 5, clientY: TOP + 1000 } as Touch)
    expect(term.scrolled).toEqual([])
    vi.advanceTimersByTime(50)
    expect(term.scrolled).toEqual([1])
    // No further events at all: the timer is the whole point.
    vi.advanceTimersByTime(150)
    expect(term.scrolled).toEqual([1, 1, 1, 1])
  })

  it("walks the other way above the top edge", () => {
    const { term, drag } = setup()
    term.lines = ["a", "b", "c", "d", "e", "git status", "g", "h"]
    term.buffer.active.viewportY = 5
    drag.begin(at(1, 0))
    drag.extend({ clientX: LEFT + 5, clientY: TOP - 1000 } as Touch)
    vi.advanceTimersByTime(50)
    expect(term.scrolled).toEqual([-1])
  })

  it("stops when the finger comes back inside", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 0))
    drag.extend({ clientX: LEFT + 5, clientY: TOP + 1000 } as Touch)
    vi.advanceTimersByTime(50)
    drag.extend(at(3, 1))
    vi.advanceTimersByTime(500)
    expect(term.scrolled).toEqual([1])
  })

  it("stops the moment the gesture ends", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 0))
    drag.extend({ clientX: LEFT + 5, clientY: TOP + 1000 } as Touch)
    drag.end()
    vi.advanceTimersByTime(500)
    expect(term.scrolled).toEqual([])
  })

  it("re-selects from the STORED finger point as the viewport moves under it", () => {
    const { term, drag } = setup()
    term.lines = ["git status", "second line", "third row here", "fourth one"]
    drag.begin(at(1, 0))
    drag.extend({ clientX: LEFT + 5, clientY: TOP + 1000 } as Touch)
    vi.advanceTimersByTime(50)
    // The viewport walked one row, so the same finger position now names a
    // different ABSOLUTE row, and the span grew by exactly one row's columns.
    expect(term.buffer.active.viewportY).toBe(1)
    expect(term.selection?.length).toBeGreaterThan(20)
  })
})

describe("ending a gesture", () => {
  it("drops the anchor but leaves the painted selection on screen", () => {
    const { term, drag } = setup()
    drag.begin(at(1, 0))
    drag.end()
    expect(drag.active()).toBe(false)
    expect(term.selection).toEqual({ col: 0, row: 0, length: 3 })
    // And a later extend cannot revive it.
    drag.extend(at(8, 0))
    expect(term.selection).toEqual({ col: 0, row: 0, length: 3 })
  })
})
