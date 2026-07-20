import { describe, expect, it } from "vitest"
import { firstFrameResizePlan } from "./firstFrameResize"

describe("firstFrameResizePlan", () => {
  it("jiggles on the very first open to repaint over the initial snapshot", () => {
    expect(firstFrameResizePlan(true)).toBe("jiggle")
  })

  it("sends a single resize on a reconnect so an unchanged size does not double-repaint", () => {
    expect(firstFrameResizePlan(false)).toBe("single")
  })
})
