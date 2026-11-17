// Pure geometry for the editor's image viewer (`ImageZoomViewer`): how big the
// picture is drawn in each mode, where it sits inside the scrollable canvas,
// and which scroll offset keeps a chosen point of the picture under a chosen
// point of the viewport while the scale changes. Everything is in CSS pixels;
// "image" coordinates are the picture's natural pixels.

export interface Size {
  width: number
  height: number
}

export interface Point {
  x: number
  y: number
}

/** Fit draws the whole picture (never above 100%), width fills the pane's
 * width, free is a scale the user chose. */
export type ZoomMode = "fit" | "width" | "free"

export const MIN_ZOOM = 0.1
export const MAX_ZOOM = 8
/** Clear space kept around the picture, matching the old frame's padding. */
export const CANVAS_PAD = 16

/** The stops the zoom buttons walk. */
const ZOOM_STOPS = [
  0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.25, 1.5, 2, 3, 4, 6, 8,
]

export function clampZoom(scale: number): number {
  if (!Number.isFinite(scale)) return 1
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, scale))
}

// Fit may go below MIN_ZOOM: showing the whole picture is its promise, and a
// very long strip needs less than 10% to keep it. This floor only stops a
// degenerate pane from producing a zero scale.
const FIT_FLOOR = 0.01

function clampFit(scale: number): number {
  return Math.min(MAX_ZOOM, Math.max(FIT_FLOOR, scale))
}

function available(viewport: number): number {
  return Math.max(1, viewport - 2 * CANVAS_PAD)
}

/** The whole picture inside the pane, never enlarged past its own pixels. */
export function fitScale(natural: Size, viewport: Size): number {
  if (natural.width <= 0 || natural.height <= 0) return 1
  return clampFit(
    Math.min(
      1,
      available(viewport.width) / natural.width,
      available(viewport.height) / natural.height,
    ),
  )
}

/** The picture as wide as the pane; it may enlarge a small picture, because
 * pressing Fit width is asking for exactly that. */
export function fitWidthScale(natural: Size, viewport: Size): number {
  if (natural.width <= 0) return 1
  return clampFit(available(viewport.width) / natural.width)
}

export function resolveScale(
  mode: ZoomMode,
  freeScale: number,
  natural: Size,
  viewport: Size,
): number {
  switch (mode) {
    case "fit":
      return fitScale(natural, viewport)
    case "width":
      return fitWidthScale(natural, viewport)
    case "free":
      return clampZoom(freeScale)
  }
}

/** The next stop above (direction 1) or below (-1) the current scale. A scale
 * between stops (from a pinch or a fit) moves to the neighbouring stop rather
 * than skipping one. */
export function stepZoom(scale: number, direction: 1 | -1): number {
  const eps = 1e-6
  if (direction > 0) {
    return ZOOM_STOPS.find((s) => s > scale + eps) ?? MAX_ZOOM
  }
  for (let i = ZOOM_STOPS.length - 1; i >= 0; i--) {
    if (ZOOM_STOPS[i] < scale - eps) return ZOOM_STOPS[i]
  }
  return MIN_ZOOM
}

/** Multiplier for one Ctrl/Cmd+wheel event. A trackpad pinch arrives as many
 * small pixel deltas and a mouse notch as one large one, so an exponential of
 * the delta treats both proportionally. Lines and pages are converted to
 * pixels first. */
export function wheelZoomFactor(deltaY: number, deltaMode: number): number {
  const pixels = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * 400 : deltaY
  // Cap one event so a coarse notch never jumps more than about 2x.
  const capped = Math.max(-350, Math.min(350, pixels))
  return Math.exp(-capped * 0.002)
}

export interface CanvasLayout {
  /** Scrollable canvas size: at least the viewport, else the picture plus padding. */
  canvas: Size
  /** The picture's drawn rectangle inside the canvas. */
  left: number
  top: number
  width: number
  height: number
  scale: number
}

export function canvasLayout(
  natural: Size,
  scale: number,
  viewport: Size,
): CanvasLayout {
  const width = natural.width * scale
  const height = natural.height * scale
  const canvas = {
    width: Math.max(viewport.width, width + 2 * CANVAS_PAD),
    height: Math.max(viewport.height, height + 2 * CANVAS_PAD),
  }
  return {
    canvas,
    left: (canvas.width - width) / 2,
    top: (canvas.height - height) / 2,
    width,
    height,
    scale,
  }
}

/** Whether the canvas is bigger than the viewport, so a drag has something to pan. */
export function canPan(layout: CanvasLayout, viewport: Size): boolean {
  return (
    layout.canvas.width > viewport.width + 0.5 ||
    layout.canvas.height > viewport.height + 0.5
  )
}

/** The picture pixel at a point of the canvas (not clamped to the picture, so
 * a point in the margin still anchors sensibly). */
export function imagePointAt(layout: CanvasLayout, canvasPoint: Point): Point {
  return {
    x: (canvasPoint.x - layout.left) / layout.scale,
    y: (canvasPoint.y - layout.top) / layout.scale,
  }
}

export function clampScroll(layout: CanvasLayout, viewport: Size, scroll: Point): Point {
  const maxX = Math.max(0, layout.canvas.width - viewport.width)
  const maxY = Math.max(0, layout.canvas.height - viewport.height)
  return {
    x: Math.min(maxX, Math.max(0, scroll.x)),
    y: Math.min(maxY, Math.max(0, scroll.y)),
  }
}

/** The scroll offset that puts picture pixel `imagePoint` at `viewportPoint`
 * (a point measured from the viewport's top-left), clamped to what the canvas
 * can scroll. Zooming around the pointer is this with the pointer as both. */
export function anchoredScroll(
  layout: CanvasLayout,
  viewport: Size,
  imagePoint: Point,
  viewportPoint: Point,
): Point {
  return clampScroll(layout, viewport, {
    x: layout.left + imagePoint.x * layout.scale - viewportPoint.x,
    y: layout.top + imagePoint.y * layout.scale - viewportPoint.y,
  })
}

export function zoomPercent(scale: number): string {
  return `${Math.round(scale * 100)}%`
}

/** The size an SVG declares on its root element (numeric width and height, or
 * else its viewBox). A browser reports a viewBox-only SVG at a made-up 150px
 * height, so the declared size is the better truth. Null when the text
 * declares neither or does not parse. */
export function svgIntrinsicSize(text: string): Size | null {
  if (typeof DOMParser === "undefined") return null
  const root = new DOMParser().parseFromString(text, "image/svg+xml").documentElement
  if (root.nodeName.toLowerCase() !== "svg") return null
  const length = (name: string) => {
    const raw = root.getAttribute(name)?.trim() ?? ""
    // Only plain or px lengths: a percentage or em says nothing on its own.
    const m = /^(\d+(?:\.\d+)?)(px)?$/.exec(raw)
    return m ? Number(m[1]) : 0
  }
  const width = length("width")
  const height = length("height")
  if (width > 0 && height > 0) return { width, height }
  const box = (root.getAttribute("viewBox") ?? "")
    .trim()
    .split(/[\s,]+/)
    .map(Number)
  if (box.length === 4 && box[2] > 0 && box[3] > 0) {
    return { width: box[2], height: box[3] }
  }
  return null
}
