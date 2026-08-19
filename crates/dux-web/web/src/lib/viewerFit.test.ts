import { describe, expect, it } from "vitest"

import {
  VIEWER_FONT_STEP,
  VIEWER_MIN_FONT_SIZE,
  viewerFontFit,
  watcherViewMode,
} from "./viewerFit"

// A 14px reference where one cell is 8x17 CSS px, close to the bundled face's
// real metrics, so the numbers below read like the real thing.
const base = {
  cell: { width: 8, height: 17 },
  referenceFontSize: 14,
  maxFontSize: 14,
}

describe("watcherViewMode", () => {
  it("reads the two modes, and everything else as the faithful default", () => {
    expect(watcherViewMode("faithful")).toBe("faithful")
    expect(watcherViewMode("fit_window")).toBe("fit_window")
    // An older server omits the field; a typo is one the server already warned
    // about and normalized. Neither may invent a third behavior here.
    expect(watcherViewMode(undefined)).toBe("faithful")
    expect(watcherViewMode(null)).toBe("faithful")
    expect(watcherViewMode("")).toBe("faithful")
    expect(watcherViewMode("Fit_Window")).toBe("faithful")
  })
})

describe("viewerFontFit", () => {
  it("keeps the user's own size when the grid already fits", () => {
    // Exactly the space 80x24 needs at 14px, to the pixel.
    const fit = viewerFontFit({
      ...base,
      available: { width: 80 * 8, height: 24 * 17 },
      grid: { rows: 24, cols: 80 },
    })
    expect(fit.fontSize).toBe(14)
    expect(fit.overflows).toBe(false)
  })

  it("never grows past the user's size, however much room there is", () => {
    const fit = viewerFontFit({
      ...base,
      available: { width: 4000, height: 4000 },
      grid: { rows: 24, cols: 80 },
    })
    expect(fit.fontSize).toBe(14)
    expect(fit.overflows).toBe(false)
  })

  it("shrinks to the largest half-pixel size that fits, and reports its pixels", () => {
    // Half the width for the same grid: the exact answer is 7px, which is on
    // the step grid and above the floor.
    const fit = viewerFontFit({
      ...base,
      available: { width: 80 * 4, height: 4000 },
      grid: { rows: 24, cols: 80 },
    })
    expect(fit.fontSize).toBe(7)
    expect(fit.overflows).toBe(false)
    expect(fit.width).toBe(80 * 4)
  })

  it("steps DOWN, never to the nearer step, because a step up would not fit", () => {
    // Room for 12.4px worth of columns. Rounding to 12.5 would overflow by a
    // fraction of a pixel per column, which is a clipped column at the far
    // side of a wide grid.
    const fit = viewerFontFit({
      ...base,
      available: { width: 80 * 8 * (12.4 / 14), height: 4000 },
      grid: { rows: 24, cols: 80 },
    })
    expect(fit.fontSize).toBe(12)
    expect(VIEWER_FONT_STEP).toBe(0.5)
  })

  it("is bound by the tighter of the two axes", () => {
    // Plenty of width, half the height.
    const fit = viewerFontFit({
      ...base,
      available: { width: 4000, height: 24 * 17 * 0.5 },
      grid: { rows: 24, cols: 80 },
    })
    expect(fit.fontSize).toBe(7)
  })

  it("clamps at the floor and SAYS it overflows rather than shrinking further", () => {
    const fit = viewerFontFit({
      ...base,
      available: { width: 200, height: 4000 },
      grid: { rows: 24, cols: 200 },
    })
    expect(fit.fontSize).toBe(VIEWER_MIN_FONT_SIZE)
    expect(fit.overflows).toBe(true)
    // The pannable area the caller must give the overflow: the grid's real
    // size at the floor font, which is wider than the window it is in.
    expect(fit.width).toBeGreaterThan(200)
  })

  it("never lets the floor GROW text past a preference below it", () => {
    const fit = viewerFontFit({
      ...base,
      maxFontSize: 6,
      available: { width: 10, height: 10 },
      grid: { rows: 24, cols: 80 },
    })
    expect(fit.fontSize).toBe(6)
    expect(fit.overflows).toBe(true)
  })

  it("answers an unmeasurable container with the user's size, not the floor", () => {
    // A pane that has not laid out yet (mount, a hidden tab). Stamping 7px on
    // it and bouncing back a frame later would be worse than waiting to be
    // asked again.
    for (const available of [
      { width: 0, height: 0 },
      { width: 500, height: 0 },
      { width: Number.NaN, height: 400 },
    ]) {
      const fit = viewerFontFit({ ...base, available, grid: { rows: 24, cols: 80 } })
      expect(fit.fontSize).toBe(14)
      expect(fit.overflows).toBe(false)
      expect(fit.width).toBe(0)
    }
  })

  it("answers an unmeasurable CELL or an empty grid the same way", () => {
    const noCell = viewerFontFit({
      ...base,
      cell: { width: 0, height: 0 },
      available: { width: 500, height: 400 },
      grid: { rows: 24, cols: 80 },
    })
    expect(noCell.fontSize).toBe(14)
    const noGrid = viewerFontFit({
      ...base,
      available: { width: 500, height: 400 },
      grid: { rows: 0, cols: 0 },
    })
    expect(noGrid.fontSize).toBe(14)
  })
})
