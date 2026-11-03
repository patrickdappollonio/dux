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
//
// The fill runs along the stroke's OUTER edge, not its centre, which is what a
// CSS border does with its own box's background and what the pill landing on
// this shape does. So the whole outline sits on the band colour: along the
// hairline (the overhangs, and the start of each fillet), where the stroke and
// the hairline are the same translucent colour and would otherwise stack into
// a brighter line, and down the sides and bottom.

/// The radius of the two concave fillets that flare the flap's top corners out
/// into the band. 12px reads at DPR 1, where a subtler arc quantizes away.
export const FLAP_FILLET_R = 12

/// The radius of the two hanging bottom corners.
export const FLAP_BOTTOM_R = 10

/// How far the fill bleeds up INTO the band, in the band's own color.
export const FLAP_BLEED = 2

/// How far the stroke runs flat along the band's hairline before each fillet,
/// plus the half pixel the side strokes sit inside the body.
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
  /// The closed body, painted in the band's color out to the stroke's outer
  /// edge, which takes in the band's hairline row it hangs from.
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
  // Every edge is stroked half a pixel inside the body, where a 1px CSS border
  // draws it, because the capsule that lands on this shape is outlined by one.
  // The corner radius is that border's centreline: its outer edge stays at
  // FLAP_BOTTOM_R.
  const r = FLAP_BOTTOM_R - 0.5
  const o = FLAP_OVERHANG

  const width = w + 2 * (f + o)
  const height = h + FLAP_BLEED + 1
  // The side strokes' centres, half a pixel inside the body's own edges.
  const xl = o + f + 0.5
  const xr = o + f + w - 0.5
  // The band hairline's center, and the bottom stroke's center. Both sit on a
  // half pixel so a 1px stroke lands on one device pixel at DPR 1.
  const y0 = FLAP_BLEED + 0.5
  const yb = FLAP_BLEED + h - 0.5
  // The stroke's outer edge, which the fill follows: the hairline's bottom,
  // the body's own edges, and the arcs half a pixel further out.
  const yh = y0 + 0.5
  const xlo = xl - 0.5
  const xro = xr + 0.5
  const ybo = yb + 0.5
  const ro = r + 0.5
  const fo = f - 0.5

  const stroke =
    `M 0 ${y0} L ${xl - f} ${y0}` +
    ` A ${f} ${f} 0 0 1 ${xl} ${y0 + f}` +
    ` L ${xl} ${yb - r}` +
    ` A ${r} ${r} 0 0 0 ${xl + r} ${yb}` +
    ` L ${xr - r} ${yb}` +
    ` A ${r} ${r} 0 0 0 ${xr} ${yb - r}` +
    ` L ${xr} ${y0 + f}` +
    ` A ${f} ${f} 0 0 1 ${xr + f} ${y0}` +
    ` L ${width} ${y0}`

  return {
    width,
    height,
    viewBox: `0 0 ${width} ${height}`,
    left: -(f + o),
    top: -FLAP_BLEED,
    fill:
      `M 0 0 L 0 ${yh} L ${xl - f} ${yh}` +
      ` A ${fo} ${fo} 0 0 1 ${xlo} ${y0 + f}` +
      ` L ${xlo} ${ybo - ro}` +
      ` A ${ro} ${ro} 0 0 0 ${xlo + ro} ${ybo}` +
      ` L ${xro - ro} ${ybo}` +
      ` A ${ro} ${ro} 0 0 0 ${xro} ${ybo - ro}` +
      ` L ${xro} ${y0 + f}` +
      ` A ${fo} ${fo} 0 0 1 ${xr + f} ${yh}` +
      ` L ${width} ${yh} L ${width} 0 Z`,
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
  const f = FLAP_FILLET_R
  // Each arc ends half a pixel inside the pill's box, on the centre of its 1px
  // border, the way the flap's arcs end on the centre of its side strokes.
  // The fill follows the arc's terminal-side edge and runs into the pill's box,
  // so the arc sits on the band colour exactly as the flap's own arcs do.
  const b = FLAP_FILLET_BOX
  const fo = f - 0.5
  if (side === "left") {
    const stroke = `M 0.5 0.5 A ${f} ${f} 0 0 1 ${f + 0.5} ${f + 0.5}`
    const fill = `M 0.5 1 A ${fo} ${fo} 0 0 1 ${f} ${f + 0.5} L ${b} ${f + 0.5} L ${b} 0 L 0.5 0 Z`
    return { stroke, fill }
  }
  const stroke = `M ${f + 0.5} 0.5 A ${f} ${f} 0 0 0 0.5 ${f + 0.5}`
  const fill = `M ${f + 0.5} 1 A ${fo} ${fo} 0 0 0 1 ${f + 0.5} L 0 ${f + 0.5} L 0 0 L ${f + 0.5} 0 Z`
  return { stroke, fill }
}
