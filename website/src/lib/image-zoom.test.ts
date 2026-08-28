import { describe, expect, it } from "vitest"

import {
  MAX_SCALE,
  MIN_SCALE,
  ZOOM_STEP,
  boundsFor,
  clampOffset,
  clampScale,
  distanceBetween,
  fitScale,
  formatZoom,
  midpoint,
  pinchScale,
  sameScale,
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

  it("snaps to natural size when a step up would cross it", () => {
    // 0.8 * 1.5 = 1.2, which would skip the one scale that means something.
    expect(stepScale(0.8, 1)).toBe(1)
    expect(stepScale(0.7, 1)).toBe(1)
  })

  it("snaps to natural size when a step down would cross it", () => {
    // 1.2 / 1.5 = 0.8.
    expect(stepScale(1.2, -1)).toBe(1)
    expect(stepScale(1.4, -1)).toBe(1)
  })

  it("steps away from natural size without sticking to it", () => {
    expect(stepScale(1, 1)).toBeCloseTo(1.5)
    expect(stepScale(1, -1)).toBeCloseTo(1 / 1.5)
  })

  it("reaches natural size from a fitted view in a few presses, exactly", () => {
    const bounds = boundsFor(0.26)
    let scale = 0.26
    const ladder: number[] = []
    for (let i = 0; i < 6 && scale !== 1; i++) {
      scale = stepScale(scale, 1, bounds)
      ladder.push(scale)
    }
    expect(ladder).toContain(1)
    expect(scale).toBe(1)
  })

  it("steps below the default floor when the fitted view is below it", () => {
    const bounds = boundsFor(0.14)
    expect(stepScale(0.21, -1, bounds)).toBeCloseTo(0.14)
    expect(stepScale(0.14, -1, bounds)).toBe(0.14)
  })

  it("never leaves the range", () => {
    expect(stepScale(MAX_SCALE, 1)).toBe(MAX_SCALE)
    expect(stepScale(MIN_SCALE, -1)).toBe(MIN_SCALE)
  })
})

describe("toggleScale", () => {
  it("goes from the fitted view to natural size", () => {
    expect(toggleScale(0.26, 0.26)).toBe(1)
  })

  it("comes back to the fitted view from natural size", () => {
    expect(toggleScale(1, 0.26)).toBe(0.26)
  })

  it("returns to natural size from any other scale", () => {
    expect(toggleScale(4, 0.26)).toBe(1)
    expect(toggleScale(0.5, 0.26)).toBe(1)
  })

  it("magnifies instead when the image already fits at natural size", () => {
    expect(toggleScale(1, 1)).toBe(2)
    expect(toggleScale(2, 1)).toBe(1)
  })
})

describe("fitScale", () => {
  it("scales a large image down until the whole of it is visible", () => {
    // 2880x1800 in a 1408x772 box: height is the binding dimension.
    expect(fitScale(2880, 1800, 1408, 772)).toBeCloseTo(772 / 1800)
    // A wide, short image is bound by width instead.
    expect(fitScale(2880, 400, 1408, 772)).toBeCloseTo(1408 / 2880)
  })

  it("never scales an image up above natural size", () => {
    expect(fitScale(726, 108, 1408, 772)).toBe(1)
    expect(fitScale(100, 100, 4000, 4000)).toBe(1)
  })

  it("fits exactly when the image is exactly the size of the box", () => {
    expect(fitScale(800, 600, 800, 600)).toBe(1)
  })

  it("falls back to natural size when a dimension is unknown", () => {
    expect(fitScale(0, 1800, 1408, 772)).toBe(1)
    expect(fitScale(2880, 1800, 0, 772)).toBe(1)
    expect(fitScale(2880, 1800, 1408, 0)).toBe(1)
  })
})

describe("boundsFor", () => {
  it("keeps the ordinary floor when the image fits comfortably", () => {
    expect(boundsFor(1)).toEqual({ min: MIN_SCALE, max: MAX_SCALE })
    expect(boundsFor(0.6)).toEqual({ min: MIN_SCALE, max: MAX_SCALE })
  })

  it("drops the floor to the fit when a huge image fits below it", () => {
    expect(boundsFor(0.13)).toEqual({ min: 0.13, max: MAX_SCALE })
  })

  it("lets clampScale hold a fitted view that is below the default floor", () => {
    expect(clampScale(0.13, boundsFor(0.13))).toBe(0.13)
    expect(clampScale(0.13)).toBe(MIN_SCALE)
  })
})

describe("sameScale", () => {
  it("treats a rounding difference as the same scale", () => {
    expect(sameScale(0.26, 0.2600001)).toBe(true)
    expect(sameScale(1, 1.2)).toBe(false)
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

  it("honours a floor lowered for a fitted view", () => {
    expect(
      zoomAtPoint({ scale: 1, x: 0, y: 0 }, 0.01, { x: 0, y: 0 }, boundsFor(0.13)).scale,
    ).toBe(0.13)
  })

  it("holds the anchor still when a double click drops back to the fitted view", () => {
    const before = { scale: 1, x: 0, y: 0 }
    const anchor = { x: -220, y: 90 }
    const after = zoomAtPoint(before, 0.26, anchor, boundsFor(0.26))
    const contentX = (anchor.x - before.x) / before.scale
    expect(after.scale).toBe(0.26)
    expect(after.x + contentX * after.scale).toBeCloseTo(anchor.x)
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

  it("cannot pinch below the fitted view", () => {
    expect(pinchScale(0.13, 200, 20, boundsFor(0.13))).toBe(0.13)
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

  it("reports a fitted view as its real fraction of natural size", () => {
    expect(formatZoom(fitScale(2880, 1800, 1408, 772))).toBe("43%")
  })
})
