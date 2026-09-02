// THE DOCKED FLAP'S SILHOUETTE, as arithmetic.
//
// The flap is a browser tab turned upside down: it grows DOWN out of the band
// above it (the agent tab strip, or the phone header when there is no strip),
// its top edge is flush with that band, two concave fillets flare its top
// corners outward into the band's own bottom hairline, and only the two hanging
// bottom corners are rounded.
//
// The whole silhouette is ONE inline SVG behind the buttons: a fill path and a
// single OPEN stroke path that never crosses the top. CSS borders plus
// pseudo-element gradient fillets could not survive real device pixel ratios:
// the 1px background-over-border overlap and the band's hairline snap to
// different device pixels at fractional scales (a visible seam), and an 8px
// gradient arc of 10%-white quantizes into nothing at DPR 1.
//
// Two tricks make the seam impossible rather than merely unlikely. The fill
// closes across the SVG's own top, `BLEED` pixels UP inside the band, in the
// band's own opaque color, so the band and the flap are one painted shape. And
// the stroke starts and ends `OVERHANG` pixels out ON the band's hairline
// before curving away, so however a DPR rounds the CSS border, the SVG stroke
// and the hairline visibly hand over to each other.
//
// It is pure geometry, so it lives here rather than in the component: the
// component measures a box, this turns the box into a path.

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
 * Build the flap's paths for a measured body box.
 *
 * `null` for a box with no size yet: a flap that has not been laid out has no
 * silhouette to draw, and guessing one paints a shape at the wrong width for a
 * frame. The caller renders no SVG until a real measurement arrives.
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
 * The two concave fillets, drawn on their own so the TRAVELLING pill can wear
 * the same corners the docked flap does.
 *
 * The pill is a capsule in flight and a tab shape at rest against the band, and
 * the morph between the two has to be the flap's actual arcs rather than a
 * gradient approximation, or the shape the flight leaves and the shape it
 * re-forms are visibly different objects.
 *
 * Each is a `FLAP_FILLET_R + 1` square hung off the pill's top corner: the
 * extra pixel is the stroke's own half-pixel offsets at both ends.
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
