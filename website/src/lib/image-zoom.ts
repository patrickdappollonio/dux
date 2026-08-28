// Pure, framework-free logic for the docs screenshot lightbox. Kept separate
// from the DOM glue in ImageZoom.astro so the arithmetic can be unit-tested
// without a browser; the component's script imports these helpers and only
// handles wiring (events, focus, transforms).
//
// The viewer's transform is always `translate(x, y) scale(s)` around the
// element's center, so every helper below works in viewport pixels measured
// from the center of the stage. Scale 1 means the image is drawn at its natural
// pixel size, which is the whole point of the lightbox: a screenshot the docs
// column had to shrink is shown at full resolution.

/** Smallest scale the controls will go to. Below this a screenshot is a smudge. */
export const MIN_SCALE = 0.25;
/** Largest scale the controls will go to. Past this it is all pixels anyway. */
export const MAX_SCALE = 8;
/** Multiplier applied by one press of Zoom in (its inverse for Zoom out). */
export const ZOOM_STEP = 1.5;
/** Where a double click lands when the viewer is sitting at 1x. */
export const DOUBLE_TAP_SCALE = 2;

/** A point in stage pixels, measured from the center of the stage. */
export interface Point {
  x: number;
  y: number;
}

/** The full viewer transform: a scale plus a translation from center. */
export interface ViewerTransform extends Point {
  scale: number;
}

/** Hold a scale inside the range the controls offer. */
export function clampScale(scale: number): number {
  if (!Number.isFinite(scale)) return 1;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

/**
 * One press of Zoom in (`direction` 1) or Zoom out (`direction` -1).
 * Multiplicative rather than additive so a step feels the same at every scale.
 */
export function stepScale(scale: number, direction: 1 | -1): number {
  return clampScale(scale * (direction === 1 ? ZOOM_STEP : 1 / ZOOM_STEP));
}

/**
 * What a double click should switch to. Anything at (or near) 1x goes to 2x;
 * anything else comes back to 1x, so a double click is always a way out of a
 * deep zoom as well as a way in.
 */
export function toggleScale(scale: number): number {
  return Math.abs(scale - 1) < 0.01 ? DOUBLE_TAP_SCALE : 1;
}

/**
 * Whether an image is worth arming with the lightbox: only one the column had
 * to SHRINK has detail to reveal. The tolerance absorbs sub-pixel layout
 * rounding, which would otherwise arm images that are effectively at 1:1.
 */
export function shouldArm(
  naturalWidth: number,
  renderedWidth: number,
  tolerance = 1,
): boolean {
  if (!(naturalWidth > 0) || !(renderedWidth > 0)) return false;
  return naturalWidth - renderedWidth > tolerance;
}

/**
 * Keep the image's translation inside the stage. An image smaller than the
 * stage is pinned to the center (offset 0); a larger one may be dragged until
 * its edge meets the stage edge, never past it.
 */
export function clampOffset(
  offset: number,
  contentSize: number,
  viewportSize: number,
): number {
  const slack = Math.max(0, (contentSize - viewportSize) / 2);
  if (!Number.isFinite(offset) || slack === 0) return 0;
  return Math.min(slack, Math.max(-slack, offset));
}

/**
 * Change scale while holding the content point under `anchor` still, which is
 * what makes wheel and pinch zoom feel anchored to the cursor or the fingers
 * rather than to the middle of the screen. `anchor` is in stage pixels from the
 * stage center; pass {x:0,y:0} for the button controls, which zoom on center.
 */
export function zoomAtPoint(
  transform: ViewerTransform,
  nextScale: number,
  anchor: Point,
): ViewerTransform {
  const scale = clampScale(nextScale);
  const ratio = scale / transform.scale;
  return {
    scale,
    x: anchor.x - (anchor.x - transform.x) * ratio,
    y: anchor.y - (anchor.y - transform.y) * ratio,
  };
}

/** Scale for a two-finger pinch, from the distance the fingers started at. */
export function pinchScale(
  startScale: number,
  startDistance: number,
  distance: number,
): number {
  if (!(startDistance > 0)) return clampScale(startScale);
  return clampScale(startScale * (distance / startDistance));
}

/** Distance between two pointers, for the pinch gesture. */
export function distanceBetween(a: Point, b: Point): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

/** Midpoint of two pointers, the anchor a pinch zooms around. */
export function midpoint(a: Point, b: Point): Point {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

/** The zoom readout in the toolbar: 1 renders as "100%". */
export function formatZoom(scale: number): string {
  return `${Math.round(scale * 100)}%`;
}
