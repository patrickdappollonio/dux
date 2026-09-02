import { readFileSync } from "node:fs"

import { describe, expect, it } from "vitest"

import {
  FLAP_BLEED,
  FLAP_BOTTOM_R,
  FLAP_FILLET_BOX,
  FLAP_FILLET_R,
  FLAP_OVERHANG,
  buildFlapShape,
  filletShape,
} from "@/lib/flapShape"

describe("the docked flap's silhouette", () => {
  it("has no shape before it has been measured", () => {
    expect(buildFlapShape({ width: 0, height: 50 })).toBeNull()
    expect(buildFlapShape({ width: 180, height: 0 })).toBeNull()
    expect(buildFlapShape({ width: -1, height: -1 })).toBeNull()
  })

  it("hangs its SVG box off the body's own top-left corner", () => {
    const shape = buildFlapShape({ width: 180, height: 50 })
    expect(shape).not.toBeNull()
    // The fillets flare out on both sides; the fill bleeds up into the band.
    expect(shape?.left).toBe(-(FLAP_FILLET_R + FLAP_OVERHANG))
    expect(shape?.top).toBe(-FLAP_BLEED)
    expect(shape?.width).toBe(180 + 2 * (FLAP_FILLET_R + FLAP_OVERHANG))
    expect(shape?.height).toBe(50 + FLAP_BLEED + 1)
    expect(shape?.viewBox).toBe(`0 0 ${shape?.width} ${shape?.height}`)
  })

  it("never strokes across the top, which is where the band is", () => {
    const shape = buildFlapShape({ width: 180, height: 50 })
    // An OPEN path: one move, and no close.
    expect(shape?.stroke.startsWith("M ")).toBe(true)
    expect(shape?.stroke).not.toContain("Z")
    // Both ends sit on the band's hairline, half a pixel down so a 1px stroke
    // lands on one device pixel.
    const y0 = FLAP_BLEED + 0.5
    expect(shape?.stroke.startsWith(`M 0 ${y0} L ${FLAP_OVERHANG} ${y0}`)).toBe(
      true,
    )
    expect(shape?.stroke.endsWith(`L ${shape.width} ${y0}`)).toBe(true)
  })

  it("closes the fill across the SVG's top, inside the band", () => {
    const shape = buildFlapShape({ width: 180, height: 50 })
    // Starts at the very top of the SVG box, which is FLAP_BLEED px up inside
    // the band, and closes back to it.
    expect(shape?.fill.startsWith("M 0 0 L")).toBe(true)
    expect(shape?.fill.endsWith(`L ${shape.width} 0 Z`)).toBe(true)
    // The body between the two is exactly the stroked outline.
    expect(shape?.fill).toContain(shape?.stroke.slice(1) as string)
  })

  it("rounds only the two hanging bottom corners", () => {
    const shape = buildFlapShape({ width: 180, height: 50 })
    const arcs = shape?.stroke.match(/A (\d+(?:\.\d+)?) /g) ?? []
    // Two fillets at the top, two rounded corners at the bottom.
    expect(arcs).toEqual([
      `A ${FLAP_FILLET_R} `,
      `A ${FLAP_BOTTOM_R} `,
      `A ${FLAP_BOTTOM_R} `,
      `A ${FLAP_FILLET_R} `,
    ])
  })

  it("grows with the cluster it is measured from", () => {
    const narrow = buildFlapShape({ width: 100, height: 50 })
    const wide = buildFlapShape({ width: 260, height: 50 })
    expect((wide?.width ?? 0) - (narrow?.width ?? 0)).toBe(160)
    // The left edge never moves: the fillet and overhang are constants.
    expect(wide?.left).toBe(narrow?.left)
  })
})

/// Every command's endpoint, and every arc's sweep flag, read back OUT of a
/// built path. Deliberately not rebuilt from the same constants the builder
/// used: a path that is wrong in the same way twice still passes a test that
/// re-derives it, and what matters about this shape is where it ends up.
function readPath(d: string) {
  const points: Array<{ x: number; y: number }> = []
  const sweeps: number[] = []
  for (const command of d.trim().split(/(?=[MLAZ])/)) {
    const verb = command[0]
    const nums = (command.slice(1).match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number)
    if (verb === "A") sweeps.push(nums[4])
    if (nums.length >= 2) {
      points.push({ x: nums[nums.length - 2], y: nums[nums.length - 1] })
    }
  }
  return { points, sweeps }
}

describe("the silhouette, read back as geometry", () => {
  it("stays inside the box it declares, corner to corner", () => {
    const shape = buildFlapShape({ width: 180, height: 50 })
    const { points } = readPath(shape?.stroke ?? "")
    const xs = points.map((p) => p.x)
    const ys = points.map((p) => p.y)
    // Nothing hangs outside the SVG, which would be clipped away on screen.
    for (const p of points) {
      expect(p.x).toBeGreaterThanOrEqual(0)
      expect(p.x).toBeLessThanOrEqual(shape?.width ?? 0)
      expect(p.y).toBeGreaterThanOrEqual(0)
      expect(p.y).toBeLessThanOrEqual(shape?.height ?? 0)
    }
    // And it spans the whole width: both ends run out onto the band's hairline.
    expect(Math.min(...xs)).toBe(0)
    expect(Math.max(...xs)).toBe(shape?.width)
    // The outline reaches the hanging bottom edge, a hairline up from the box.
    expect(Math.max(...ys)).toBeGreaterThan((shape?.height ?? 0) / 2)
    expect(Math.max(...ys)).toBeLessThan(shape?.height ?? 0)
  })

  it("curves the top corners the opposite way from the bottom ones", () => {
    // THE FILLETS ARE CONCAVE and the hanging corners convex, which is the
    // whole difference between a tab growing out of the band and a box stuck
    // under it. In SVG that is the sweep flag, and it is the one thing about
    // this path that a wrong radius cannot show up as.
    const shape = buildFlapShape({ width: 180, height: 50 })
    expect(readPath(shape?.stroke ?? "").sweeps).toEqual([1, 0, 0, 1])
  })
})

describe("the fillets the travelling pill wears", () => {
  it("draws a square one pixel bigger than the arc, for the stroke's offsets", () => {
    expect(FLAP_FILLET_BOX).toBe(FLAP_FILLET_R + 1)
  })

  it("hangs off the pill at exactly the radius it draws", () => {
    // The offsets are a stylesheet's, the arc is this module's, and nothing
    // else makes the two agree: a fillet parked at the wrong offset leaves a
    // gap between the arc and the capsule it is supposed to grow out of.
    const css = readFileSync("src/index.css", "utf8")
    const left = /\.dux-pill-fillet-l \{\s*left: (-?\d+)px/.exec(css)
    const right = /\.dux-pill-fillet-r \{\s*right: (-?\d+)px/.exec(css)
    expect(left).not.toBeNull()
    expect(right).not.toBeNull()
    expect(Number(left?.[1])).toBe(-FLAP_FILLET_R)
    expect(Number(right?.[1])).toBe(-FLAP_FILLET_R)
  })

  it("mirrors left and right, and closes each fill back into the corner", () => {
    const left = filletShape("left")
    const right = filletShape("right")
    expect(left.stroke).toBe(`M 0 0.5 A 12 12 0 0 1 12 12.5`)
    expect(right.stroke).toBe(`M 13 0.5 A 12 12 0 0 0 1 12.5`)
    expect(left.fill).toBe(`${left.stroke} L 12 0 Z`)
    expect(right.fill).toBe(`${right.stroke} L 1 0 Z`)
  })
})
