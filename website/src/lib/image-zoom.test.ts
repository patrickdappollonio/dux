import { describe, expect, it } from "vitest"

import {
  MAX_SCALE,
  MIN_SCALE,
  ZOOM_STEP,
  clampOffset,
  clampScale,
  distanceBetween,
  formatZoom,
  midpoint,
  pinchScale,
  shouldArm,
  stepScale,
  toggleScale,
  zoomAtPoint,
} from "./image-zoom"

describe("clampScale", () => {
  it("passes a scale inside the range through", () => {
    expect(clampScale(1)).toBe(1)
    expect(clampScale(3.5)).toBe(3.5)
  })

  it("holds the ends", () => {
    expect(clampScale(0.01)).toBe(MIN_SCALE)
    expect(clampScale(500)).toBe(MAX_SCALE)
  })

  it("falls back to 1 for a non-finite scale", () => {
    expect(clampScale(Number.NaN)).toBe(1)
    expect(clampScale(Number.POSITIVE_INFINITY)).toBe(1)
  })
})

describe("stepScale", () => {
  it("multiplies in and divides out by the same factor", () => {
    expect(stepScale(1, 1)).toBeCloseTo(ZOOM_STEP)
    expect(stepScale(ZOOM_STEP, -1)).toBeCloseTo(1)
  })

  it("compounds across presses", () => {
    expect(stepScale(stepScale(1, 1), 1)).toBeCloseTo(ZOOM_STEP * ZOOM_STEP)
  })

  it("never leaves the range", () => {
    expect(stepScale(MAX_SCALE, 1)).toBe(MAX_SCALE)
    expect(stepScale(MIN_SCALE, -1)).toBe(MIN_SCALE)
  })
})

describe("toggleScale", () => {
  it("goes to 2x from 1x", () => {
    expect(toggleScale(1)).toBe(2)
  })

  it("comes back to 1x from anywhere else", () => {
    expect(toggleScale(2)).toBe(1)
    expect(toggleScale(0.5)).toBe(1)
    expect(toggleScale(6)).toBe(1)
  })
})

describe("shouldArm", () => {
  it("arms an image the column shrank", () => {
    expect(shouldArm(2880, 720)).toBe(true)
  })

  it("leaves an image drawn at its natural width alone", () => {
    expect(shouldArm(726, 726)).toBe(false)
  })

  it("absorbs sub-pixel layout rounding", () => {
    expect(shouldArm(726, 725.4)).toBe(false)
    expect(shouldArm(726, 700)).toBe(true)
  })

  it("refuses an image that has not loaded", () => {
    expect(shouldArm(0, 720)).toBe(false)
    expect(shouldArm(2880, 0)).toBe(false)
  })
})

describe("clampOffset", () => {
  it("pins content smaller than the stage to the center", () => {
    expect(clampOffset(120, 400, 800)).toBe(0)
    expect(clampOffset(-120, 400, 800)).toBe(0)
  })

  it("allows panning up to the edge of larger content", () => {
    expect(clampOffset(50, 1000, 800)).toBe(50)
    expect(clampOffset(500, 1000, 800)).toBe(100)
    expect(clampOffset(-500, 1000, 800)).toBe(-100)
  })

  it("recenters a non-finite offset", () => {
    expect(clampOffset(Number.NaN, 1000, 800)).toBe(0)
  })
})

describe("zoomAtPoint", () => {
  it("zooms on center when the anchor is the center", () => {
    expect(zoomAtPoint({ scale: 1, x: 0, y: 0 }, 2, { x: 0, y: 0 })).toEqual({
      scale: 2,
      x: 0,
      y: 0,
    })
  })

  it("holds the content point under the anchor still", () => {
    const before = { scale: 1, x: 0, y: 0 }
    const anchor = { x: 100, y: -40 }
    const after = zoomAtPoint(before, 2, anchor)
    // The content point under the anchor is (anchor - x) / scale; after the
    // zoom it must still land on the anchor.
    const contentX = (anchor.x - before.x) / before.scale
    const contentY = (anchor.y - before.y) / before.scale
    expect(after.x + contentX * after.scale).toBeCloseTo(anchor.x)
    expect(after.y + contentY * after.scale).toBeCloseTo(anchor.y)
  })

  it("clamps the scale it is handed", () => {
    expect(zoomAtPoint({ scale: 1, x: 0, y: 0 }, 999, { x: 0, y: 0 }).scale).toBe(
      MAX_SCALE,
    )
  })
})

describe("pinchScale", () => {
  it("scales with the ratio the fingers moved", () => {
    expect(pinchScale(1, 100, 200)).toBe(2)
    expect(pinchScale(2, 200, 100)).toBe(1)
  })

  it("survives a zero starting distance", () => {
    expect(pinchScale(1.5, 0, 200)).toBe(1.5)
  })
})

describe("pointer geometry", () => {
  it("measures distance and midpoint", () => {
    expect(distanceBetween({ x: 0, y: 0 }, { x: 3, y: 4 })).toBe(5)
    expect(midpoint({ x: 0, y: 0 }, { x: 4, y: 10 })).toEqual({ x: 2, y: 5 })
  })
})

describe("formatZoom", () => {
  it("reads natural size as 100%", () => {
    expect(formatZoom(1)).toBe("100%")
    expect(formatZoom(2.25)).toBe("225%")
    expect(formatZoom(0.5)).toBe("50%")
  })
})
