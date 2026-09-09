// The docked flap's silhouette, as arithmetic: the component measures a box,
// this turns the box into a path.
//
// The flap is an upside-down browser tab hanging off the band above it, with a
// top edge flush to that band, concave fillets flaring its top corners into the
// band's hairline, and only its bottom corners rounded. The whole silhouette is
// one inline SVG, a fill path plus a single open stroke path that never crosses
// the top: CSS borders and gradient fillets seam at fractional device pixel
// ratios, where the border overlap and the band's hairline snap to different
// device pixels and a faint gradient arc quantizes away.
//
// Two rules keep the seam impossible. The fill closes across the SVG's own top,
// `FLAP_BLEED` pixels up inside the band and in the band's opaque color, so band
// and flap are one painted shape; the stroke starts and ends `FLAP_OVERHANG`
// pixels out on the hairline before curving away, so however a DPR rounds the
// CSS border, stroke and hairline hand over to each other.

/// The radius of the two concave fillets that flare the flap's top corners out
/// into the band. 12px reads at DPR 1, where a subtler arc quantizes away.
export const FLAP_FILLET_R = 12

/// The radius of the two hanging bottom corners.
export const FLAP_BOTTOM_R = 10

/// How far the fill bleeds up INTO the band, in the band's own color.
export const FLAP_BLEED = 2

/// How far the stroke runs flat along the band's hairline before each fillet.
export const FLAP_OVERHANG = 3

/** Everything the component needs to paint one flap. */
export interface FlapShape {
  /// The SVG's own box, which is wider and taller than the flap's body: the
  /// fillets hang off both sides and the fill bleeds up into the band.
  width: number
  height: number
  viewBox: string
  /// Where that box sits relative to the flap's own top-left corner. Both are
  /// negative: the SVG starts up and to the left of the body it draws.
  left: number
  top: number
  /// The closed body, painted in the band's color.
  fill: string
  /// The open outline: fillets, sides and bottom corners, and nothing across
  /// the top, because the flap has no top edge.
  stroke: string
}

/**
 * Build the flap's paths for a measured body box, or `null` for a box with no
 * size yet, since guessing one paints a wrong-width shape for a frame. The
 * caller renders no SVG until a real measurement arrives.
 */
export function buildFlapShape(
  body: { width: number; height: number },
): FlapShape | null {
  const w = body.width
  const h = body.height
  if (!(w > 0) || !(h > 0)) return null

  const f = FLAP_FILLET_R
  const r = FLAP_BOTTOM_R
  const o = FLAP_OVERHANG

  const width = w + 2 * (f + o)
  const height = h + FLAP_BLEED + 1
  // The body's own side edges, inside the wider SVG box.
  const xl = o + f
  const xr = xl + w
  // The band hairline's center, and the bottom stroke's center. Both sit on a
  // half pixel so a 1px stroke lands on one device pixel at DPR 1.
  const y0 = FLAP_BLEED + 0.5
  const yb = FLAP_BLEED + h - 0.5

  const stroke =
    `M 0 ${y0} L ${o} ${y0}` +
    ` A ${f} ${f} 0 0 1 ${xl} ${y0 + f}` +
    ` L ${xl} ${yb - r}` +
    ` A ${r} ${r} 0 0 0 ${xl + r} ${yb}` +
    ` L ${xr - r} ${yb}` +
    ` A ${r} ${r} 0 0 0 ${xr} ${yb - r}` +
    ` L ${xr} ${y0 + f}` +
    ` A ${f} ${f} 0 0 1 ${width - o} ${y0}` +
    ` L ${width} ${y0}`

  return {
    width,
    height,
    viewBox: `0 0 ${width} ${height}`,
    left: -(f + o),
    top: -FLAP_BLEED,
    fill: `M 0 0 L${stroke.slice(1)} L ${width} 0 Z`,
    stroke,
  }
}

/**
 * The two concave fillets drawn on their own, so the travelling pill morphs
 * through the flap's actual arcs rather than an approximation the flight would
 * leave and re-form as a visibly different shape. Each is a `FLAP_FILLET_R + 1`
 * square hung off a top corner, the extra pixel being the stroke's half-pixel
 * offsets at both ends.
 */
export const FLAP_FILLET_BOX = FLAP_FILLET_R + 1

export interface FilletShape {
  fill: string
  stroke: string
}

export function filletShape(side: "left" | "right"): FilletShape {
  const b = FLAP_FILLET_BOX
  const f = FLAP_FILLET_R
  if (side === "left") {
    const stroke = `M 0 0.5 A ${f} ${f} 0 0 1 ${f} ${f + 0.5}`
    return { stroke, fill: `${stroke} L ${f} 0 Z` }
  }
  const stroke = `M ${b} 0.5 A ${f} ${f} 0 0 0 1 ${f + 0.5}`
  return { stroke, fill: `${stroke} L 1 0 Z` }
}
