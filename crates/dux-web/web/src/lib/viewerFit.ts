// The pure half of the watcher's faithful view. One PTY has one grid, the
// owner's, and a watcher whose grid differs renders wrapped, clamped output
// into its own scrollback. The re-grid to the PTY's rows and columns belongs to
// the resize coordinator; this module answers the other half, the font size
// that makes that grid fit the window.
//
// The shrink is a font size and never a CSS `scale()`: selection, hyperlink
// resolution and forwarded touch gestures resolve a cell by dividing a
// client-space rect by the grid, and a transformed element reports the scaled
// rect that xterm's own hit-testing disagrees with. Changing the font moves the
// real cell metrics, so those paths keep working with no special case.
//
// Deliberately free of any xterm, React or DOM import, so the arithmetic is
// testable without a layout. The caller measures.

/// The smallest font the faithful view will shrink to, in CSS pixels: a
/// legibility judgement, since the bundled faces stop resolving strokes below
/// it on an ordinary display. A grid that does not fit at this size is left
/// overflowing its container, and the pane makes the overflow pannable. There
/// is no preference to escape it with.
export const VIEWER_MIN_FONT_SIZE = 7

/// The granularity of the shrink, in CSS pixels. Half steps rather than whole
/// ones because a whole-pixel search wastes up to a pixel of cell height on
/// every row, which on a 50-row grid is a whole row's worth of window.
export const VIEWER_FONT_STEP = 0.5

export type ViewerFitInput = {
  /// The space the terminal may occupy, in CSS pixels, with any scrollbar
  /// gutter ALREADY subtracted by the caller (the caller is the one that knows
  /// which gutters exist).
  available: { width: number; height: number }
  /// The grid to render: the PTY's own, never this window's.
  grid: { rows: number; cols: number }
  /// One cell's size, in CSS pixels, MEASURED at `referenceFontSize`. Cell
  /// metrics are a font-relative measurement, so one measurement answers for
  /// every candidate size.
  cell: { width: number; height: number }
  /// The font size the measurement above was taken at.
  referenceFontSize: number
  /// The user's own terminal font size. The shrink never grows past it: a
  /// watcher on a huge monitor sees the agent's grid at the size they chose,
  /// not blown up to fill the window.
  maxFontSize: number
}

export type ViewerFitResult = {
  /// The font size to apply, in CSS pixels.
  fontSize: number
  /// True when even the floor font does not fit, so the caller must let the
  /// terminal overflow and make that overflow reachable.
  overflows: boolean
  /// The grid's rendered size at `fontSize`, in CSS pixels. The caller uses it
  /// to size the pannable area in the overflow case; it is meaningless (zero)
  /// when nothing could be measured.
  width: number
  height: number
}

function positive(value: number): boolean {
  return Number.isFinite(value) && value > 0
}

/**
 * The largest font size at which `grid` fits inside `available`, in half-pixel
 * steps, never above `maxFontSize` and never below [`VIEWER_MIN_FONT_SIZE`].
 *
 * An unmeasured container (no layout yet, a backgrounded tab, a `display: none`
 * parent) reports zero, and answers with the user's own size rather than the
 * floor, waiting to be asked again on the caller's resize observation.
 */
export function viewerFontFit(input: ViewerFitInput): ViewerFitResult {
  const { available, grid, cell, referenceFontSize, maxFontSize } = input
  const measured =
    positive(available.width) &&
    positive(available.height) &&
    positive(cell.width) &&
    positive(cell.height) &&
    positive(referenceFontSize) &&
    positive(maxFontSize) &&
    grid.rows > 0 &&
    grid.cols > 0
  if (!measured) {
    return { fontSize: maxFontSize, overflows: false, width: 0, height: 0 }
  }
  // Cell size per pixel of font size. xterm rounds cells to whole device
  // pixels, so this errs by under a device pixel toward a smaller font.
  const perFontWidth = cell.width / referenceFontSize
  const perFontHeight = cell.height / referenceFontSize
  // Each ratio is a font size, not a scale: the grid at font `f` is
  // `perFont * f * count` wide, so dividing the space out yields `f`.
  const ideal = Math.min(
    available.width / (perFontWidth * grid.cols),
    available.height / (perFontHeight * grid.rows),
  )
  const stepped =
    Math.floor(Math.min(ideal, maxFontSize) / VIEWER_FONT_STEP) *
    VIEWER_FONT_STEP
  // A preference below the floor would otherwise make the floor GROW the text.
  const floor = Math.min(VIEWER_MIN_FONT_SIZE, maxFontSize)
  const overflows = stepped < floor
  const fontSize = overflows ? floor : stepped
  return {
    fontSize,
    overflows,
    width: Math.ceil(perFontWidth * fontSize * grid.cols),
    height: Math.ceil(perFontHeight * fontSize * grid.rows),
  }
}
