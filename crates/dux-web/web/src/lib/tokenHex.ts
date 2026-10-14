// The sRGB hex (six digits, no `#`) a CSS color token paints as, for APIs that
// take nothing but hex, such as Monaco's theme rules. The theme tokens are
// oklch and `getComputedStyle` hands a custom property back as written, so the
// value is painted onto a 1x1 canvas and the pixel read back, which leaves the
// conversion and gamut mapping to the browser that paints the rest of the page.
// With no stylesheet or no canvas (jsdom), or a value the canvas cannot parse,
// the fallback is returned.
export function tokenHex(property: string, fallback: string): string {
  if (typeof document === "undefined") return fallback
  const value = getComputedStyle(document.documentElement)
    .getPropertyValue(property)
    .trim()
  if (value === "") return fallback
  let ctx: CanvasRenderingContext2D | null
  try {
    const canvas = document.createElement("canvas")
    canvas.width = 1
    canvas.height = 1
    ctx = canvas.getContext("2d", { willReadFrequently: true })
  } catch {
    return fallback
  }
  if (!ctx) return fallback
  // A value the canvas cannot parse is ignored, leaving the fallback to paint.
  ctx.fillStyle = `#${fallback}`
  ctx.fillStyle = value
  ctx.fillRect(0, 0, 1, 1)
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data
  return [r, g, b]
    .map((c) => c.toString(16).padStart(2, "0"))
    .join("")
    .toUpperCase()
}
