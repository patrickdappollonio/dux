// THE WATCHER'S FAITHFUL VIEW: the pure half.
//
// ONE PTY HAS ONE GRID, the owner's. A watcher renders the same byte stream
// into its own xterm, so a watcher whose grid differs is looking at wrapped,
// clamped output, and every repaint the child makes scrolls mangled rows into
// that watcher's LOCAL scrollback, where they stay until a fresh attach. The
// badge and the bounce-heal (`components/terminal/viewerGrid.ts`) treat the
// symptom; this module is how the divergence is removed instead.
//
// THE ANSWER IS PRESENTATION, NOT GEOMETRY. The watcher's emulator is
// re-gridded to the PTY's real rows and columns (that half is the resize
// coordinator's, because it is a re-grid and every re-grid is the
// coordinator's), and then the FONT is shrunk until that grid fits the window.
// The picture is then byte-for-byte what the driver sees, just smaller.
//
// WHY A FONT AND NOT A CSS TRANSFORM. A `scale()` on the terminal would be one
// line and would break xterm's pixel-to-cell arithmetic everywhere it matters:
// selection, hyperlink resolution and the forwarded touch gestures all resolve
// a cell by dividing a client-space rect by the grid, and a transformed element
// reports the SCALED rect while xterm's own hit-testing does not agree with it.
// Changing the font size moves the real cell metrics, so every one of those
// paths keeps working with no special case.
//
// Deliberately free of any xterm/React/DOM import, so the arithmetic is
// testable without a layout (see `viewerFit.test.ts`). The caller measures.

/// The smallest font the faithful view will shrink to, in CSS pixels.
///
/// Below this the text is not small, it is gone: at 6px and under the bundled
/// faces stop resolving strokes on an ordinary display, so a "faithful" view
/// would be faithful to nothing a human can read. When the grid does not fit
/// at this size the terminal is left OVERFLOWING its container and the pane
/// makes the overflow pannable, which is an honest answer (the picture is
/// still correct, you scroll to the rest of it) where an illegible one is not.
/// Chosen rather than measured: it is a legibility judgement. There is no
/// preference to escape it with any more: `ui.watcher_view` was removed with
/// the badge, because the full-pane take-over card hid the only difference the
/// two modes ever had, and the pannable overflow below the floor is the honest
/// answer for a window too small to hold the driver's grid.
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
 * NOTHING MEASURED IS NOT "SHRINK EVERYTHING". A container with no layout yet
 * (the frame before mount lays out, a backgrounded tab, a pane whose parent is
 * `display: none`) reports zero, and answering that with the floor font would
 * stamp 7px text on the terminal and bounce back a frame later. It answers
 * with the user's own size instead and waits to be asked again, which the
 * caller's resize observation guarantees.
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
  // Cell size per pixel of font size. xterm rounds the cell to whole device
  // pixels, so this is very slightly approximate; the error is under one
  // device pixel per cell and always in the direction of a marginally smaller
  // font, which is the harmless direction.
  const perFontWidth = cell.width / referenceFontSize
  const perFontHeight = cell.height / referenceFontSize
  // Each ratio is already a font SIZE, not a scale: the grid at font `f` is
  // `perFont * f * count` wide, so the `f` that exactly fills the space is the
  // space divided by `perFont * count`.
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
