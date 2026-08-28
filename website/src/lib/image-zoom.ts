// Pure, framework-free logic for the docs screenshot lightbox. Kept separate
// from the DOM glue in ImageZoom.astro so the arithmetic can be unit-tested
// without a browser; the component's script imports these helpers and only
// handles wiring (events, focus, transforms).
//
// The viewer's transform is always `translate(x, y) scale(s)` around the
// element's center, so every helper below works in viewport pixels measured
// from the center of the stage. Scale 1 means the image is drawn at its natural
// pixel size, and the readout says so: it is a percentage of natural size, not
// of the fitted view, so a reader always knows whether they are looking at real
// pixels.
//
// The viewer opens FITTED (the whole screenshot visible), because a screenshot
// is a picture of a layout before it is a wall of pixels, and landing on a crop
// of the middle answers no question the reader had. Natural size is one double
// click away, and one or two zoom steps away, because the step ladder snaps to
// 100% whenever a step would cross it.

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
 * fits below the ordinary floor, and the floor must not fight the fit: zooming
 * all the way out has to be able to show the whole image.
 */
export function boundsFor(fit: number): ScaleBounds {
  return { min: Math.min(MIN_SCALE, fit), max: MAX_SCALE };
}

/**
 * The scale at which the whole image is visible in the space available. Never
 * above 1: a small screenshot is shown at its own size rather than blown up
 * into a blurry poster.
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
 * Multiplicative, so a step feels the same at every scale, with one exception:
 * a step that would cross natural size lands ON natural size instead. That
 * makes 100% a stop on the ladder rather than a value you can only skip past,
 * which matters because 100% is the one scale that means something.
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
 * What a double click should switch to: fitted and natural size, back and
 * forth. When the image already fits at natural size there is no third state
 * to offer, so a double click magnifies instead.
 */
export function toggleScale(scale: number, fit: number): number {
  if (sameScale(fit, 1)) return sameScale(scale, 1) ? DOUBLE_TAP_SCALE : 1;
  return sameScale(scale, 1) ? fit : 1;
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
