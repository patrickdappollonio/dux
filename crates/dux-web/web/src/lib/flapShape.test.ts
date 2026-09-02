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

describe("the fillets the travelling pill wears", () => {
  it("draws a square one pixel bigger than the arc, for the stroke's offsets", () => {
    expect(FLAP_FILLET_BOX).toBe(FLAP_FILLET_R + 1)
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
