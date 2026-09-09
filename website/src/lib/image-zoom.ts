// Pure, framework-free logic for the docs screenshot lightbox, kept apart from
// the DOM glue in ImageZoom.astro so the arithmetic is unit-testable.
//
// The viewer's transform is always `translate(x, y) scale(s)` around the
// element's center, so every helper works in viewport pixels measured from the
// stage center, and scale 1 is natural pixel size. The readout is a percentage of
// natural size, never of the fitted view. The viewer opens fitted, with natural
// size a double click or a snapping zoom step away.

/** Smallest scale the controls step down to; a fitted view may sit below it. */
export const MIN_SCALE = 0.25;
/** Largest scale the controls will go to. Past this it is all pixels anyway. */
export const MAX_SCALE = 8;
/** Multiplier applied by one press of Zoom in (its inverse for Zoom out). */
export const ZOOM_STEP = 1.5;
/** Where a double click lands when fitting and natural size are the same thing. */
export const DOUBLE_TAP_SCALE = 2;
/** Scales this close together are the same scale to a reader. */
export const SCALE_EPSILON = 0.005;

/** A point in stage pixels, measured from the center of the stage. */
export interface Point {
  x: number;
  y: number;
}

/** The full viewer transform: a scale plus a translation from center. */
export interface ViewerTransform extends Point {
  scale: number;
}

/** The scale range the controls may move through. */
export interface ScaleBounds {
  min: number;
  max: number;
}

const DEFAULT_BOUNDS: ScaleBounds = { min: MIN_SCALE, max: MAX_SCALE };

/**
 * The bounds for a given fitted scale. A screenshot far larger than the window
 * fits below the ordinary floor, and zooming all the way out must still show the
 * whole image, so the floor may not fight the fit.
 */
export function boundsFor(fit: number): ScaleBounds {
  return { min: Math.min(MIN_SCALE, fit), max: MAX_SCALE };
}

/**
 * The scale at which the whole image is visible in the space available. Never
 * above 1, so a small screenshot is shown at its own size.
 */
export function fitScale(
  naturalWidth: number,
  naturalHeight: number,
  availableWidth: number,
  availableHeight: number,
): number {
  if (!(naturalWidth > 0) || !(naturalHeight > 0)) return 1;
  if (!(availableWidth > 0) || !(availableHeight > 0)) return 1;
  return Math.min(1, availableWidth / naturalWidth, availableHeight / naturalHeight);
}

/** Whether two scales are the same scale as far as the reader is concerned. */
export function sameScale(a: number, b: number): boolean {
  return Math.abs(a - b) < SCALE_EPSILON;
}

/** Hold a scale inside the range the controls offer. */
export function clampScale(scale: number, bounds: ScaleBounds = DEFAULT_BOUNDS): number {
  if (!Number.isFinite(scale)) return 1;
  return Math.min(bounds.max, Math.max(bounds.min, scale));
}

/**
 * One press of Zoom in (`direction` 1) or Zoom out (`direction` -1).
 * Multiplicative, so a step feels the same at every scale, except that a step
 * crossing natural size lands on it: 100% is a stop, not a value to skip past.
 */
export function stepScale(
  scale: number,
  direction: 1 | -1,
  bounds: ScaleBounds = DEFAULT_BOUNDS,
): number {
  const target = scale * (direction === 1 ? ZOOM_STEP : 1 / ZOOM_STEP);
  const crossesNatural =
    (scale < 1 - SCALE_EPSILON && target > 1) || (scale > 1 + SCALE_EPSILON && target < 1);
  if (crossesNatural) return 1;
  return clampScale(target, bounds);
}

/**
 * What a double click switches to: fitted and natural size, back and forth. An
 * image already fitting at natural size has no third state, so it magnifies.
 */
export function toggleScale(scale: number, fit: number): number {
  if (sameScale(fit, 1)) return sameScale(scale, 1) ? DOUBLE_TAP_SCALE : 1;
  return sameScale(scale, 1) ? fit : 1;
}

/**
 * Whether an image is worth arming with the lightbox: only one the column had to
 * shrink has detail to reveal. The tolerance absorbs sub-pixel layout rounding,
 * which would otherwise arm images effectively at 1:1.
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
 * Keep the image's translation inside the stage: one smaller than the stage is
 * pinned to the center, a larger one may be dragged until its edge meets the
 * stage edge, never past it.
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
 * Change scale while holding the content point under `anchor` still, so wheel and
 * pinch zoom track the cursor or the fingers. `anchor` is in stage pixels from
 * the stage center; the button controls pass {x:0,y:0} and zoom on center.
 */
export function zoomAtPoint(
  transform: ViewerTransform,
  nextScale: number,
  anchor: Point,
  bounds: ScaleBounds = DEFAULT_BOUNDS,
): ViewerTransform {
  const scale = clampScale(nextScale, bounds);
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
  bounds: ScaleBounds = DEFAULT_BOUNDS,
): number {
  if (!(startDistance > 0)) return clampScale(startScale, bounds);
  return clampScale(startScale * (distance / startDistance), bounds);
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
