import { afterEach, describe, expect, it, vi } from "vitest"

import {
  THEATER_PILL_HINT_KEY,
  THEATER_PILL_MARGIN,
  THEATER_PILL_NUDGE_PX,
  THEATER_PILL_POSITION_KEY,
  classifyPillGesture,
  clampPillPosition,
  defaultPillPosition,
  nudgePillPosition,
  parsePillPosition,
  readPillPosition,
  readPillHintPending,
  markPillHintShown,
  resolvePillPosition,
  writePillPosition,
} from "./theaterPill"

function memoryStorage(initial: Record<string, string> = {}) {
  const mem = new Map(Object.entries(initial))
  return {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
    mem,
  }
}

const surface = { width: 800, height: 600 }
const pill = { width: 200, height: 48 }

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("where the pill sits", () => {
  it("defaults to the bottom-right corner, one margin off each edge", () => {
    expect(defaultPillPosition(surface, pill)).toEqual({
      x: 800 - 200 - THEATER_PILL_MARGIN,
      y: 600 - 48 - THEATER_PILL_MARGIN,
    })
  })

  it("keeps the default on screen when the surface is smaller than the pill", () => {
    expect(defaultPillPosition({ width: 100, height: 20 }, pill)).toEqual({
      x: 0,
      y: 0,
    })
  })

  it("clamps to the left and top edges", () => {
    expect(clampPillPosition({ x: -40, y: -9 }, surface, pill)).toEqual({
      x: 0,
      y: 0,
    })
  })

  it("clamps to the right and bottom edges", () => {
    expect(clampPillPosition({ x: 5000, y: 5000 }, surface, pill)).toEqual({
      x: 600,
      y: 552,
    })
  })

  it("clamps a corner overshoot on both axes at once", () => {
    expect(clampPillPosition({ x: -50, y: 5000 }, surface, pill)).toEqual({
      x: 0,
      y: 552,
    })
  })

  it("leaves a position already inside the surface alone", () => {
    expect(clampPillPosition({ x: 120, y: 30 }, surface, pill)).toEqual({
      x: 120,
      y: 30,
    })
  })

  it("re-clamps into a surface that just shrank, which is a rotation", () => {
    const landscape = clampPillPosition({ x: 600, y: 552 }, surface, pill)
    expect(clampPillPosition(landscape, { width: 390, height: 844 }, pill)).toEqual({
      x: 190,
      y: 552,
    })
  })

  it("pins the pill to the origin when the surface cannot hold it", () => {
    expect(clampPillPosition({ x: 40, y: 40 }, { width: 10, height: 10 }, pill)).toEqual({
      x: 0,
      y: 0,
    })
  })
})

describe("resolving the position a freshly mounted pill takes", () => {
  it("uses the stored position, clamped into the current surface", () => {
    expect(resolvePillPosition({ x: 700, y: 10 }, surface, pill)).toEqual({
      x: 600,
      y: 10,
    })
  })

  it("falls back to the default corner with nothing stored", () => {
    expect(resolvePillPosition(null, surface, pill)).toEqual(
      defaultPillPosition(surface, pill),
    )
  })

  it("refuses to answer for a surface nothing has measured yet", () => {
    expect(resolvePillPosition(null, { width: 0, height: 0 }, pill)).toBeNull()
  })
})

describe("nudging the pill with the arrow keys", () => {
  const at = { x: 100, y: 100 }

  it("moves one step per press, clamped like a drag", () => {
    expect(nudgePillPosition(at, "ArrowLeft", surface, pill)).toEqual({
      x: 100 - THEATER_PILL_NUDGE_PX,
      y: 100,
    })
    expect(nudgePillPosition(at, "ArrowRight", surface, pill)).toEqual({
      x: 100 + THEATER_PILL_NUDGE_PX,
      y: 100,
    })
    expect(nudgePillPosition(at, "ArrowUp", surface, pill)).toEqual({
      x: 100,
      y: 100 - THEATER_PILL_NUDGE_PX,
    })
    expect(nudgePillPosition(at, "ArrowDown", surface, pill)).toEqual({
      x: 100,
      y: 100 + THEATER_PILL_NUDGE_PX,
    })
  })

  it("stops at the edge rather than walking off it", () => {
    expect(nudgePillPosition({ x: 4, y: 0 }, "ArrowLeft", surface, pill)).toEqual({
      x: 0,
      y: 0,
    })
  })

  it("ignores every other key", () => {
    expect(nudgePillPosition(at, "Enter", surface, pill)).toBeNull()
    expect(nudgePillPosition(at, "a", surface, pill)).toBeNull()
  })
})

describe("telling a tap from a drag", () => {
  it("lifts a mouse press that has travelled past the threshold", () => {
    expect(
      classifyPillGesture({ pointerType: "mouse", heldMs: 10, travel: 6, ended: false }),
    ).toBe("lift")
  })

  it("waits while a mouse press has barely moved", () => {
    expect(
      classifyPillGesture({ pointerType: "mouse", heldMs: 900, travel: 5, ended: false }),
    ).toBe("pending")
  })

  it("treats a mouse press that never travelled as a tap", () => {
    expect(
      classifyPillGesture({ pointerType: "mouse", heldMs: 80, travel: 2, ended: true }),
    ).toBe("tap")
  })

  it("lifts a finger that has been held long enough", () => {
    expect(
      classifyPillGesture({ pointerType: "touch", heldMs: 300, travel: 2, ended: false }),
    ).toBe("lift")
  })

  it("waits while a finger is still early in its hold", () => {
    expect(
      classifyPillGesture({ pointerType: "touch", heldMs: 299, travel: 2, ended: false }),
    ).toBe("pending")
  })

  it("cancels a finger that slid away before the hold completed, which is a scroll", () => {
    expect(
      classifyPillGesture({ pointerType: "touch", heldMs: 100, travel: 9, ended: false }),
    ).toBe("cancel")
  })

  it("treats a short finger press as a tap", () => {
    expect(
      classifyPillGesture({ pointerType: "touch", heldMs: 120, travel: 1, ended: true }),
    ).toBe("tap")
  })

  it("gives a pen the mouse's travel gate, since it points", () => {
    expect(
      classifyPillGesture({ pointerType: "pen", heldMs: 10, travel: 7, ended: false }),
    ).toBe("lift")
  })
})

describe("the remembered position", () => {
  it("parses a stored pair", () => {
    expect(parsePillPosition('{"x":12,"y":34}')).toEqual({ x: 12, y: 34 })
  })

  it("rejects nothing stored, junk, and the wrong shape", () => {
    expect(parsePillPosition(null)).toBeNull()
    expect(parsePillPosition("")).toBeNull()
    expect(parsePillPosition("not json")).toBeNull()
    expect(parsePillPosition("[1,2]")).toBeNull()
    expect(parsePillPosition('{"x":"12","y":34}')).toBeNull()
    expect(parsePillPosition('{"x":12}')).toBeNull()
  })

  it("rejects values no surface could produce", () => {
    expect(parsePillPosition('{"x":-1,"y":10}')).toBeNull()
    expect(parsePillPosition('{"x":10,"y":-1}')).toBeNull()
    expect(parsePillPosition('{"x":1e9,"y":10}')).toBeNull()
    expect(parsePillPosition('{"x":null,"y":10}')).toBeNull()
  })

  it("reads and writes one key for the whole device", () => {
    const store = memoryStorage()
    vi.stubGlobal("localStorage", store)
    writePillPosition({ x: 21, y: 42 })
    expect(store.mem.get(THEATER_PILL_POSITION_KEY)).toBe('{"x":21,"y":42}')
    expect(readPillPosition()).toEqual({ x: 21, y: 42 })
  })

  it("rounds to whole pixels on the way in", () => {
    const store = memoryStorage()
    vi.stubGlobal("localStorage", store)
    writePillPosition({ x: 21.6, y: 42.2 })
    expect(readPillPosition()).toEqual({ x: 22, y: 42 })
  })

  it("survives a storage that throws on every access", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked")
      },
      setItem: () => {
        throw new Error("blocked")
      },
      removeItem: () => {
        throw new Error("blocked")
      },
    })
    expect(readPillPosition()).toBeNull()
    expect(() => writePillPosition({ x: 1, y: 2 })).not.toThrow()
  })
})

describe("the one-time drag hint", () => {
  it("is pending on a device that has never seen it, then never again", () => {
    const store = memoryStorage()
    vi.stubGlobal("localStorage", store)
    expect(readPillHintPending()).toBe(true)
    markPillHintShown()
    expect(store.mem.get(THEATER_PILL_HINT_KEY)).toBe("shown")
    expect(readPillHintPending()).toBe(false)
  })

  it("stays quiet when storage cannot answer, rather than nagging every mount", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked")
      },
      setItem: () => {
        throw new Error("blocked")
      },
      removeItem: () => {
        throw new Error("blocked")
      },
    })
    expect(readPillHintPending()).toBe(false)
    expect(() => markPillHintShown()).not.toThrow()
  })
})
