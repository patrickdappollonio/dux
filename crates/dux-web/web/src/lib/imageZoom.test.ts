// @vitest-environment jsdom
import { describe, expect, it } from "vitest"

import {
  anchoredScroll,
  canPan,
  canvasLayout,
  CANVAS_PAD,
  fitScale,
  fitWidthScale,
  imagePointAt,
  MAX_ZOOM,
  MIN_ZOOM,
  stepZoom,
  svgIntrinsicSize,
  wheelZoomFactor,
} from "@/lib/imageZoom"

const pane = { width: 832, height: 632 } // 800x600 inside the padding

describe("fit scales", () => {
  it("fit shrinks a tall screenshot to the pane's height", () => {
    expect(fitScale({ width: 400, height: 6000 }, pane)).toBeCloseTo(0.1)
  })

  it("fit never enlarges a small picture", () => {
    expect(fitScale({ width: 32, height: 32 }, pane)).toBe(1)
  })

  it("fit width fills the width whatever the height, enlarging if needed", () => {
    expect(fitWidthScale({ width: 400, height: 6000 }, pane)).toBe(2)
    expect(fitWidthScale({ width: 1600, height: 100 }, pane)).toBe(0.5)
  })

  it("fit goes below the zoom floor to keep a very long strip whole", () => {
    expect(fitScale({ width: 10, height: 100000 }, pane)).toBeLessThan(MIN_ZOOM)
    expect(fitWidthScale({ width: 1, height: 1 }, pane)).toBe(MAX_ZOOM)
  })
})

describe("stepZoom", () => {
  it("moves to the neighbouring stop from an in-between scale", () => {
    expect(stepZoom(1, 1)).toBe(1.25)
    expect(stepZoom(1, -1)).toBe(0.9)
    expect(stepZoom(0.4, 1)).toBe(0.5)
    expect(stepZoom(0.4, -1)).toBe(0.33)
  })

  it("stops at the bounds", () => {
    expect(stepZoom(MAX_ZOOM, 1)).toBe(MAX_ZOOM)
    expect(stepZoom(MIN_ZOOM, -1)).toBe(MIN_ZOOM)
  })
})

describe("wheelZoomFactor", () => {
  it("zooms in on a wheel up and out on a wheel down, symmetrically", () => {
    const up = wheelZoomFactor(-100, 0)
    const down = wheelZoomFactor(100, 0)
    expect(up).toBeGreaterThan(1)
    expect(up * down).toBeCloseTo(1)
  })

  it("treats a line-mode notch like its pixel equivalent and caps a big jump", () => {
    expect(wheelZoomFactor(3, 1)).toBeCloseTo(wheelZoomFactor(48, 0))
    expect(wheelZoomFactor(-100000, 0)).toBeLessThan(2.1)
  })
})

describe("layout and anchoring", () => {
  const natural = { width: 400, height: 300 }

  it("centres a picture smaller than the pane and leaves nothing to pan", () => {
    const layout = canvasLayout(natural, 1, pane)
    expect(layout.canvas).toEqual(pane)
    expect(layout.left).toBe(216)
    expect(layout.top).toBe(166)
    expect(canPan(layout, pane)).toBe(false)
  })

  it("pads a picture bigger than the pane and lets it pan", () => {
    const layout = canvasLayout(natural, 4, pane)
    expect(layout.canvas).toEqual({
      width: 1600 + 2 * CANVAS_PAD,
      height: 1200 + 2 * CANVAS_PAD,
    })
    expect(layout.left).toBe(CANVAS_PAD)
    expect(canPan(layout, pane)).toBe(true)
  })

  it("keeps the picture pixel under the pointer when zooming in", () => {
    const pointer = { x: 500, y: 350 }
    const before = canvasLayout(natural, 1, pane)
    const pixel = imagePointAt(before, pointer) // no scroll yet
    const after = canvasLayout(natural, 3, pane)
    const scroll = anchoredScroll(after, pane, pixel, pointer)
    const back = imagePointAt(after, { x: pointer.x + scroll.x, y: pointer.y + scroll.y })
    expect(back.x).toBeCloseTo(pixel.x)
    expect(back.y).toBeCloseTo(pixel.y)
  })

  it("clamps the scroll at the canvas edges", () => {
    const layout = canvasLayout(natural, 4, pane)
    expect(anchoredScroll(layout, pane, { x: 0, y: 0 }, { x: 400, y: 300 })).toEqual({
      x: 0,
      y: 0,
    })
    expect(
      anchoredScroll(layout, pane, { x: 400, y: 300 }, { x: 0, y: 0 }),
    ).toEqual({
      x: layout.canvas.width - pane.width,
      y: layout.canvas.height - pane.height,
    })
  })
})

describe("svgIntrinsicSize", () => {
  it("reads numeric width and height, else the viewBox", () => {
    expect(svgIntrinsicSize('<svg xmlns="http://www.w3.org/2000/svg" width="40px" height="20"/>')).toEqual({
      width: 40,
      height: 20,
    })
    expect(
      svgIntrinsicSize('<svg xmlns="http://www.w3.org/2000/svg" width="100%" viewBox="0 0 24 12"/>'),
    ).toEqual({ width: 24, height: 12 })
  })

  it("returns null for text that is not an svg", () => {
    expect(svgIntrinsicSize("<svg")).toBeNull()
    expect(svgIntrinsicSize('<svg xmlns="http://www.w3.org/2000/svg"/>')).toBeNull()
  })
})
