import { describe, expect, it } from "vitest"

import { nextAppliedGeneration, shouldApplyReplay } from "./replayGeneration"

describe("shouldApplyReplay", () => {
  it("applies the first replay of a socket lifetime (nothing applied yet)", () => {
    expect(shouldApplyReplay(1, null)).toBe(true)
  })

  it("applies a strictly newer generation", () => {
    expect(shouldApplyReplay(5, 4)).toBe(true)
  })

  it("drops a duplicate of the generation already applied", () => {
    expect(shouldApplyReplay(4, 4)).toBe(false)
  })

  it("drops a stale, older generation (late blob from a torn-down forwarder)", () => {
    expect(shouldApplyReplay(3, 7)).toBe(false)
  })

  it("applies when the server sent no generation (backward-safe, null)", () => {
    expect(shouldApplyReplay(null, 9)).toBe(true)
  })

  it("applies when the server sent no generation (backward-safe, undefined)", () => {
    expect(shouldApplyReplay(undefined, 9)).toBe(true)
  })
})

describe("nextAppliedGeneration", () => {
  it("advances the high-water mark to a tagged generation", () => {
    expect(nextAppliedGeneration(6, 4)).toBe(6)
  })

  it("leaves the mark unchanged for an untagged replay (null)", () => {
    expect(nextAppliedGeneration(null, 4)).toBe(4)
  })

  it("leaves the mark unchanged for an untagged replay (undefined)", () => {
    expect(nextAppliedGeneration(undefined, 4)).toBe(4)
  })

  it("seeds the mark from the first tagged generation", () => {
    expect(nextAppliedGeneration(2, null)).toBe(2)
  })

  it("models the drop-then-keep sequence a duplicate reconnect produces", () => {
    // First replay of a lifetime: applied, mark seeded.
    let mark: number | null = null
    expect(shouldApplyReplay(10, mark)).toBe(true)
    mark = nextAppliedGeneration(10, mark)
    expect(mark).toBe(10)
    // A duplicate blob at the same generation must be dropped and NOT move the mark.
    expect(shouldApplyReplay(10, mark)).toBe(false)
    // A genuine newer reconnect still applies and advances the mark.
    expect(shouldApplyReplay(11, mark)).toBe(true)
    mark = nextAppliedGeneration(11, mark)
    expect(mark).toBe(11)
  })
})
