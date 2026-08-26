// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest"

// jsdom does not expose localStorage as a bare global here, and the helpers
// under test read it directly.
const mem = new Map<string, string>()
vi.stubGlobal("localStorage", {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, String(v)),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

import {
  DIVIDER_CHROME,
  DIVIDER_HIT_SLOP,
  DIVIDER_STORAGE_KEYS,
  DIVIDER_TARGET_MIN,
  dividerCursor,
  dividerHitBand,
  dividerKeyAction,
  dividerPressHits,
  readStoredPanePercent,
  readStoredText,
  withinDividerBand,
  writeStoredText,
} from "./paneDivider"

const rect = (left: number, right: number) => ({
  left,
  right,
  top: 0,
  bottom: 100,
})

describe("dividerHitBand", () => {
  it("grows a hair-thin divider to the coarse minimum, centred on the line", () => {
    const band = dividerHitBand(rect(200, 201), DIVIDER_TARGET_MIN.coarse)
    expect(band.right - band.left).toBe(DIVIDER_TARGET_MIN.coarse)
    expect((band.left + band.right) / 2).toBe(200.5)
  })

  it("grows to the smaller fine minimum for a mouse", () => {
    const band = dividerHitBand(rect(200, 204), DIVIDER_TARGET_MIN.fine)
    expect(band.right - band.left).toBe(DIVIDER_TARGET_MIN.fine)
  })

  it("leaves a divider that is already wide enough alone", () => {
    const band = dividerHitBand(rect(200, 240), DIVIDER_TARGET_MIN.coarse)
    expect(band.left).toBe(200)
    expect(band.right).toBe(240)
  })

  it("never widens the long axis", () => {
    const band = dividerHitBand(rect(200, 201), DIVIDER_TARGET_MIN.coarse)
    expect(band.top).toBe(0)
    expect(band.bottom).toBe(100)
  })
})

// The one acquisition rule both dividers use, so a press that lands on one
// lands on the other.
describe("dividerPressHits", () => {
  function divider(left: number, right: number): HTMLElement {
    const el = document.createElement("div")
    el.getBoundingClientRect = () =>
      new DOMRect(left, 0, right - left, 100) as DOMRect
    document.body.append(el)
    return el
  }

  const press = (target: EventTarget | null, x: number, y = 50) => ({
    target,
    clientX: x,
    clientY: y,
  })

  it("takes a press inside the grown band", () => {
    const el = divider(200, 201)
    expect(
      dividerPressHits(el, press(document.body, 209), DIVIDER_TARGET_MIN.coarse),
    ).toBe(true)
  })

  it("refuses a press outside the band that landed on something else", () => {
    const el = divider(200, 201)
    expect(
      dividerPressHits(el, press(document.body, 260), DIVIDER_TARGET_MIN.coarse),
    ).toBe(false)
  })

  // The browser adjusts a touch point before it dispatches, so a press it has
  // already given to the divider can arrive with coordinates well outside the
  // band. That verdict wins; the strip between the two widths used to be dead.
  it("takes a press the browser has already given to the divider", () => {
    const el = divider(200, 201)
    expect(dividerPressHits(el, press(el, 260), DIVIDER_TARGET_MIN.coarse)).toBe(
      true,
    )
  })

  it("takes a press on something inside the divider", () => {
    const el = divider(200, 201)
    const child = document.createElement("span")
    el.append(child)
    expect(
      dividerPressHits(el, press(child, 400), DIVIDER_TARGET_MIN.coarse),
    ).toBe(true)
  })

  it("refuses everything when there is no divider yet", () => {
    expect(
      dividerPressHits(null, press(document.body, 200), DIVIDER_TARGET_MIN.fine),
    ).toBe(false)
  })
})

describe("withinDividerBand", () => {
  const band = dividerHitBand(rect(200, 201), DIVIDER_TARGET_MIN.coarse)

  it("accepts a press anywhere inside the band, including its edges", () => {
    expect(withinDividerBand(band, 190.5, 50)).toBe(true)
    expect(withinDividerBand(band, 200.5, 0)).toBe(true)
    expect(withinDividerBand(band, 210.5, 100)).toBe(true)
  })

  it("refuses a press outside the band on either axis", () => {
    expect(withinDividerBand(band, 189, 50)).toBe(false)
    expect(withinDividerBand(band, 212, 50)).toBe(false)
    expect(withinDividerBand(band, 200.5, 101)).toBe(false)
  })
})

describe("the shared chrome", () => {
  // The grab band is written as literal Tailwind classes because Tailwind
  // scans source text, so nothing can build them from the constants. This is
  // what keeps the two from drifting apart.
  it("states the same widths the hit band is computed from", () => {
    expect(DIVIDER_HIT_SLOP).toContain(`after:w-[${DIVIDER_TARGET_MIN.fine}px]`)
    expect(DIVIDER_HIT_SLOP).toContain(
      `pointer-coarse:after:w-[${DIVIDER_TARGET_MIN.coarse}px]`,
    )
  })

  it("suppresses touch-action across the whole band", () => {
    expect(DIVIDER_CHROME).toContain("touch-none")
    expect(DIVIDER_CHROME).toContain(DIVIDER_HIT_SLOP)
  })
})

describe("dividerKeyAction", () => {
  // The action says which way and whether it is a nudge or a run to the end.
  // How far a nudge goes is the caller's, because the two dividers deliberately
  // disagree: see the note on SidebarDragEdge's keydown handler.
  it("nudges either way on the arrows", () => {
    expect(dividerKeyAction("ArrowLeft")).toEqual({
      kind: "step",
      direction: -1,
      toEnd: false,
    })
    expect(dividerKeyAction("ArrowRight")).toEqual({
      kind: "step",
      direction: 1,
      toEnd: false,
    })
  })

  it("runs the divider to its ends on Home and End", () => {
    expect(dividerKeyAction("Home")).toEqual({
      kind: "step",
      direction: -1,
      toEnd: true,
    })
    expect(dividerKeyAction("End")).toEqual({
      kind: "step",
      direction: 1,
      toEnd: true,
    })
  })

  it("toggles the collapse on Enter", () => {
    expect(dividerKeyAction("Enter")).toEqual({ kind: "toggle" })
  })

  it("ignores everything else, so typing never moves a divider", () => {
    expect(dividerKeyAction("a")).toBeNull()
    expect(dividerKeyAction("ArrowUp")).toBeNull()
    expect(dividerKeyAction(" ")).toBeNull()
  })
})

describe("dividerCursor", () => {
  it("matches what the panel library paints per engine", () => {
    expect(dividerCursor("Mozilla/5.0 Chrome/130")).toBe("ew-resize")
    expect(dividerCursor("Mozilla/5.0 Firefox/130")).toBe("ew-resize")
    expect(dividerCursor("Mozilla/5.0 Version/17 Safari/605")).toBe(
      "col-resize",
    )
  })
})

describe("readStoredPanePercent", () => {
  const key = DIVIDER_STORAGE_KEYS.changesPanePercent

  it("reads a remembered split back", () => {
    localStorage.setItem(key, "37.5")
    expect(readStoredPanePercent(key, 26, 14, 70)).toBe(37.5)
  })

  it("falls back when nothing was ever written", () => {
    localStorage.removeItem(key)
    expect(readStoredPanePercent(key, 26, 14, 70)).toBe(26)
  })

  it("falls back rather than restoring a pane too narrow to use", () => {
    localStorage.setItem(key, "0")
    expect(readStoredPanePercent(key, 26, 14, 70)).toBe(26)
    localStorage.setItem(key, "13.9")
    expect(readStoredPanePercent(key, 26, 14, 70)).toBe(26)
  })

  it("falls back rather than squeezing the neighbour under its own floor", () => {
    localStorage.setItem(key, "70")
    expect(readStoredPanePercent(key, 26, 14, 70)).toBe(70)
    localStorage.setItem(key, "70.1")
    expect(readStoredPanePercent(key, 26, 14, 70)).toBe(26)
  })

  it("falls back on a hand-edited entry it cannot read", () => {
    localStorage.setItem(key, "wide please")
    expect(readStoredPanePercent(key, 26, 14, 70)).toBe(26)
    localStorage.setItem(key, "400")
    expect(readStoredPanePercent(key, 26, 14, 70)).toBe(26)
  })
})

// A browser in private mode, with site data blocked, or over quota throws on
// both reads and writes. Losing the remembered size is the whole cost; a
// divider that throws on release is not.
describe("storage that throws", () => {
  const key = DIVIDER_STORAGE_KEYS.sidebarWidth

  function withHostileStorage(run: () => void) {
    const previous = globalThis.localStorage
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new DOMException("denied", "SecurityError")
      },
      setItem: () => {
        throw new DOMException("denied", "SecurityError")
      },
    })
    try {
      run()
    } finally {
      vi.stubGlobal("localStorage", previous)
    }
  }

  it("reads as absent instead of throwing", () => {
    withHostileStorage(() => {
      expect(readStoredText(key)).toBeNull()
      expect(readStoredPanePercent(key, 26, 14, 70)).toBe(26)
    })
  })

  it("swallows a refused write", () => {
    withHostileStorage(() => {
      expect(() => writeStoredText(key, "20rem")).not.toThrow()
    })
  })
})
