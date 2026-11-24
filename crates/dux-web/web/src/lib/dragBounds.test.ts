import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { SORT_MODIFIERS, clampToBounds, restrictToScrollContainer } from "@/lib/dragBounds"
import { sourceFilesUnder } from "@/test/sourceFiles"

const rect = (top: number, left: number, width: number, height: number) => ({
  top,
  left,
  width,
  height,
  right: left + width,
  bottom: top + height,
})

const LIST = rect(100, 0, 200, 300)
const ROW = rect(150, 10, 180, 50)

describe("clampToBounds", () => {
  it("leaves a drag that stays inside alone", () => {
    expect(clampToBounds({ x: 5, y: 40 }, ROW, LIST)).toEqual({ x: 5, y: 40 })
  })

  it("stops the row's box at the list's bottom edge, however far the pointer goes", () => {
    for (const y of [1, 40, 650, 100000]) {
      const out = clampToBounds({ x: 0, y: 150 + y }, ROW, LIST)
      expect(ROW.bottom + out.y).toBeLessThanOrEqual(LIST.bottom)
    }
    expect(clampToBounds({ x: 0, y: 5000 }, ROW, LIST).y).toBe(LIST.bottom - ROW.bottom)
  })

  it("stops the row's box at the list's top edge", () => {
    expect(clampToBounds({ x: 0, y: -5000 }, ROW, LIST).y).toBe(LIST.top - ROW.top)
  })

  it("holds the sides too, so a sideways drag cannot widen the scroller", () => {
    expect(clampToBounds({ x: 900, y: 0 }, ROW, LIST).x).toBe(LIST.right - ROW.right)
    expect(clampToBounds({ x: -900, y: 0 }, ROW, LIST).x).toBe(LIST.left - ROW.left)
  })

  it("keeps the other fields of the transform", () => {
    expect(clampToBounds({ x: 0, y: 5000, scaleX: 1, scaleY: 1 }, ROW, LIST)).toMatchObject({
      scaleX: 1,
      scaleY: 1,
    })
  })

  it("pins a box taller than the bounds by its leading edge", () => {
    const tall = rect(100, 10, 180, 400)
    expect(clampToBounds({ x: 0, y: 300 }, tall, LIST).y).toBe(0)
  })
})

describe("restrictToScrollContainer", () => {
  const args = (over: object) =>
    ({
      transform: { x: 0, y: 5000, scaleX: 1, scaleY: 1 },
      draggingNodeRect: ROW,
      scrollableAncestorRects: [LIST],
      ...over,
    }) as unknown as Parameters<typeof restrictToScrollContainer>[0]

  it("clamps against the first scrolling ancestor", () => {
    expect(restrictToScrollContainer(args({})).y).toBe(LIST.bottom - ROW.bottom)
  })

  it("passes the drag through when nothing scrolls or nothing is measured", () => {
    expect(restrictToScrollContainer(args({ scrollableAncestorRects: [] })).y).toBe(5000)
    expect(restrictToScrollContainer(args({ draggingNodeRect: null })).y).toBe(5000)
  })
})

// A sortable list that skips the bound brings the runaway back, so every
// DndContext in the app has to carry it. Reads text, not the program.
describe("every drag context bounds its drag", () => {
  const root = fileURLToPath(new URL("../", import.meta.url))
  const users = sourceFilesUnder(root).filter((p) => readFileSync(p, "utf8").includes("<DndContext"))

  it("finds the lists that reorder by drag", () => {
    expect(users.map((p) => p.split("/").pop()).sort()).toEqual([
      "FlatAgentList.tsx",
      "MacrosDialog.tsx",
      "PullRequestsDialog.tsx",
      "SortableStrip.tsx",
    ])
  })

  it.each(users)("%s passes SORT_MODIFIERS", (path) => {
    expect(readFileSync(path, "utf8")).toMatch(/modifiers=\{SORT_MODIFIERS\}/)
  })

  it("exports the one modifier", () => {
    expect(SORT_MODIFIERS).toEqual([restrictToScrollContainer])
  })
})
