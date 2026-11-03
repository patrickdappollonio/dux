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
import { THEATER_PILL_ROW_GAP_PX } from "@/lib/theaterPill"

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
    expect(
      shape?.stroke.startsWith(`M 0 ${y0} L ${FLAP_OVERHANG + 0.5} ${y0}`),
    ).toBe(true)
    expect(shape?.stroke.endsWith(`L ${shape.width} ${y0}`)).toBe(true)
  })

  it("closes the fill across the SVG's top, inside the band", () => {
    const shape = buildFlapShape({ width: 180, height: 50 })
    // Starts at the very top of the SVG box, which is FLAP_BLEED px up inside
    // the band, and closes back to it.
    expect(shape?.fill.startsWith("M 0 0 L")).toBe(true)
    expect(shape?.fill.endsWith(`L ${shape.width} 0 Z`)).toBe(true)
  })

  it("paints its body colour under the whole outline, as a CSS border's box does", () => {
    // The pill that lands on this shape draws its 1px border over its own
    // background, so the flap's outline has to sit wholly on the fill too, out
    // to the body's edge and no further.
    const shape = buildFlapShape({ width: 180, height: 50 })
    const fill = shape?.fill ?? ""
    const bodyLeft = FLAP_FILLET_R + FLAP_OVERHANG
    const bodyRight = bodyLeft + 180
    const bodyBottom = FLAP_BLEED + 50
    const midY = FLAP_BLEED + 25
    const midX = bodyLeft + 90
    expect(filled(fill, bodyLeft + 0.25, midY)).toBe(true)
    expect(filled(fill, bodyLeft - 0.25, midY)).toBe(false)
    expect(filled(fill, bodyRight - 0.25, midY)).toBe(true)
    expect(filled(fill, bodyRight + 0.25, midY)).toBe(false)
    expect(filled(fill, midX, bodyBottom - 0.25)).toBe(true)
    expect(filled(fill, midX, bodyBottom + 0.25)).toBe(false)
  })

  it("rounds only the two hanging bottom corners", () => {
    const shape = buildFlapShape({ width: 180, height: 50 })
    const arcs = shape?.stroke.match(/A (\d+(?:\.\d+)?) /g) ?? []
    // Two fillets at the top, two rounded corners at the bottom. A corner's
    // radius is its stroke's centreline, half a pixel inside the outer edge,
    // which is where a 1px CSS border of `FLAP_BOTTOM_R` draws it too.
    expect(arcs).toEqual([
      `A ${FLAP_FILLET_R} `,
      `A ${FLAP_BOTTOM_R - 0.5} `,
      `A ${FLAP_BOTTOM_R - 0.5} `,
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

/// Whether a point lies inside any closed subpath of `d`, each read as the
/// polygon through its endpoints. Arcs become chords, which is exact for the
/// straight runs this is asked about.
function filled(d: string, x: number, y: number): boolean {
  return d
    .split(/(?=M)/)
    .map((sub) => readPath(sub).points)
    .some((poly) => {
      let inside = false
      for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
        const a = poly[i]
        const b = poly[j]
        if (a.y > y !== b.y > y && x < ((b.x - a.x) * (y - a.y)) / (b.y - a.y) + a.x) {
          inside = !inside
        }
      }
      return inside
    })
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

  it("lays its own fill under every stroke pixel on the band's hairline", () => {
    // The stroke and the band's hairline are both the translucent border
    // colour, so wherever the stroke runs over a bare hairline the two stack
    // into a brighter line. The hairline is the 1px row the flap's top sits on
    // (`-top-px`), which in SVG units starts FLAP_BLEED down.
    const shape = buildFlapShape({ width: 180, height: 50 })
    const fill = shape?.fill ?? ""
    const width = shape?.width ?? 0
    const inHairline = FLAP_BLEED + 0.75
    const belowHairline = FLAP_BLEED + 1.25
    // The overhangs, where the stroke lies flat along the hairline.
    expect(filled(fill, 1, inHairline)).toBe(true)
    expect(filled(fill, width - 1, inHairline)).toBe(true)
    // Never past it: below the hairline is the terminal.
    expect(filled(fill, 1, belowHairline)).toBe(false)
    expect(filled(fill, width - 1, belowHairline)).toBe(false)
  })

  it("strokes its sides where the pill's CSS border draws them", () => {
    // The capsule that lands on this shape is outlined by a 1px CSS border,
    // which lies INSIDE its box; a side stroke centred on the body's edge sits
    // half a pixel further out and visibly steps at the swap. The bottom has
    // always been inside, so the sides join it.
    const shape = buildFlapShape({ width: 180, height: 50 })
    const { points } = readPath(shape?.stroke ?? "")
    const bodyLeft = FLAP_FILLET_R + FLAP_OVERHANG
    const bodyRight = bodyLeft + 180
    const bodyBottom = FLAP_BLEED + 50
    const xs = new Set(points.map((p) => p.x))
    expect(xs.has(bodyLeft + 0.5)).toBe(true)
    expect(xs.has(bodyRight - 0.5)).toBe(true)
    expect(xs.has(bodyLeft)).toBe(false)
    expect(xs.has(bodyRight)).toBe(false)
    expect(Math.max(...points.map((p) => p.y))).toBe(bodyBottom - 0.5)
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
    //
    // An absolutely placed child is measured from the pill's PADDING box, one
    // border width inside the box the docked flap's arcs are measured from, so
    // the border is added back on both axes or the arcs land a pixel low and a
    // pixel in at the swap.
    const pillBorderPx = 1
    const css = readFileSync("src/index.css", "utf8")
    const top = /\.dux-pill-fillet \{[^}]*\btop: (-?\d+)(?:px)?;/.exec(css)
    const left = /\.dux-pill-fillet-l \{\s*left: (-?\d+)px/.exec(css)
    const right = /\.dux-pill-fillet-r \{\s*right: (-?\d+)px/.exec(css)
    expect(top).not.toBeNull()
    expect(left).not.toBeNull()
    expect(right).not.toBeNull()
    expect(Number(top?.[1])).toBe(-pillBorderPx)
    expect(Number(left?.[1])).toBe(-(FLAP_FILLET_R + pillBorderPx))
    expect(Number(right?.[1])).toBe(-(FLAP_FILLET_R + pillBorderPx))
  })

  it("lands its arcs on the flap's, in the box the two of them share", () => {
    // The pill parks on the flap's box, so in that box's own coordinates the
    // pill's arcs, placed by the stylesheet, must coincide with the flap's or
    // the swap moves them. The pill's fillets are placed from its padding box,
    // one border width inside.
    const width = 180
    const border = 1
    const css = readFileSync("src/index.css", "utf8")
    const top = Number(/\.dux-pill-fillet \{[^}]*\btop: (-?\d+)px;/.exec(css)?.[1])
    const left = Number(/\.dux-pill-fillet-l \{\s*left: (-?\d+)px/.exec(css)?.[1])
    const right = Number(/\.dux-pill-fillet-r \{\s*right: (-?\d+)px/.exec(css)?.[1])
    const shape = buildFlapShape({ width, height: 50 })
    expect(shape).not.toBeNull()
    const shift = (pts: Array<{ x: number; y: number }>, dx: number, dy: number) =>
      pts.map((pt) => ({ x: pt.x + dx, y: pt.y + dy }))
    const flap = shift(
      readPath(shape?.stroke ?? "").points,
      shape?.left ?? 0,
      shape?.top ?? 0,
    )
    const leftFillet = readPath(filletShape("left").stroke)
    const rightFillet = readPath(filletShape("right").stroke)
    const rightSvgLeft = width - border - right - FLAP_FILLET_BOX
    // The flap's outline is: in, left fillet, down, corner, across, corner, up,
    // right fillet, out. The pill's right fillet is drawn top to bottom, the
    // flap's bottom to top, so its sweep flag is the mirror of the flap's.
    expect(shift(leftFillet.points, border + left, border + top)).toEqual([
      flap[1],
      flap[2],
    ])
    expect(shift(rightFillet.points, rightSvgLeft, border + top)).toEqual([
      flap[8],
      flap[7],
    ])
    expect(leftFillet.sweeps).toEqual([1])
    expect(rightFillet.sweeps).toEqual([0])
  })

  it("collapses the grip slot so it advances the row by nothing", () => {
    // The pill parks on the flap's box only if the collapsed grip adds nothing:
    // its zero width, its two border pixels, its margin and the row's gap
    // together.
    const css = readFileSync("src/index.css", "utf8")
    const rule = /\.dux-pill-gripless \.dux-pill-grip \{([^}]*)\}/.exec(css)?.[1] ?? ""
    const length = (prop: string) =>
      Number(new RegExp(`(?:^|[;\\s])${prop}: (-?\\d+)(?:px)?;`).exec(rule)?.[1])
    const gripBorderPx = 1
    expect(
      length("width") + 2 * gripBorderPx + length("margin-right") + THEATER_PILL_ROW_GAP_PX,
    ).toBe(0)
  })

  it("never changes the grip's border, which is half its focus ring", () => {
    // A border width in flight is snapped to whole device pixels, so it sits at
    // one per side and then hops; and the border is where the keyboard focus
    // ring's solid line is drawn. Nothing about the grip slot touches it.
    const css = readFileSync("src/index.css", "utf8")
    const gripRules = [...css.matchAll(/[^{}]*\.dux-pill-grip[^{}]*\{([^}]*)\}/g)].map(
      (m) => m[1],
    )
    expect(gripRules.length).toBeGreaterThan(0)
    for (const body of gripRules) expect(body).not.toMatch(/\bborder/)
  })

  it("mirrors left and right, and closes each fill back into the corner", () => {
    const left = filletShape("left")
    const right = filletShape("right")
    expect(left.stroke).toBe(`M 0.5 0.5 A 12 12 0 0 1 12.5 12.5`)
    expect(right.stroke).toBe(`M 12.5 0.5 A 12 12 0 0 0 0.5 12.5`)
    // Each fill runs along the arc's terminal-side edge, half a pixel in from
    // the stroke's centre, so the arc sits on the band colour the way the
    // flap's own arcs do.
    expect(left.fill).toBe(`M 0.5 1 A 11.5 11.5 0 0 1 12 12.5 L 13 12.5 L 13 0 L 0.5 0 Z`)
    expect(right.fill).toBe(`M 12.5 1 A 11.5 11.5 0 0 0 1 12.5 L 0 12.5 L 0 0 L 12.5 0 Z`)
  })
})
