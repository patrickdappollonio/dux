// Pure geometry for the desktop sidebar's drag-to-resize edge: the sidebar's own
// band, which the Changes pane has no equivalent of. The gesture itself is shared,
// in lib/paneDivider.ts and hooks/use-divider-drag.ts.

// The expanded sidebar's resizable band.
export const MIN_SIDEBAR_PX = 14 * 16
export const MAX_SIDEBAR_PX = 28 * 16
// Below this released width the sidebar auto-collapses to the icon rail: it is the
// width below which the list rows stop being readable.
export const AUTO_COLLAPSE_SIDEBAR_PX = 15 * 16

// What the sidebar is worth before anyone drags it, and what a double-click on
// the edge puts it back to (the Changes divider's double-click resets to its
// own default the same way).
export const DEFAULT_SIDEBAR_WIDTH = "18rem"
export const DEFAULT_SIDEBAR_PX = 18 * 16

// One arrow key press. Deliberately NOT the panel library's 5% of the group:
// see the note on SidebarDragEdge's keydown handler for why a percentage of
// the window collapses the whole sidebar in one press on an ordinary screen.
export const SIDEBAR_KEY_STEP_PX = 16

// Read a stored or in-flight sidebar width back into pixels. dux only ever
// writes `<n>rem`, but a hand-edited localStorage entry reaches this too, so an
// unreadable value lands on the default rather than on NaN.
export function sidebarWidthToPx(width: string): number {
  const match = /^\s*(-?[\d.]+)\s*(rem|px)?\s*$/.exec(width)
  if (!match) return DEFAULT_SIDEBAR_PX
  const value = Number.parseFloat(match[1])
  if (!Number.isFinite(value)) return DEFAULT_SIDEBAR_PX
  return match[2] === "px" ? value : value * 16
}

// Decides the outcome of a resize-handle release: the candidate width clamped to the
// band, plus whether it should auto-collapse. Every gesture that can change the width
// goes through this, so none of them can disagree about the band.
export function sidebarResizeRelease(px: number): {
  widthRem: string
  collapse: boolean
} {
  const clamped = Math.min(Math.max(px, MIN_SIDEBAR_PX), MAX_SIDEBAR_PX)
  return {
    widthRem: `${clamped / 16}rem`,
    collapse: clamped < AUTO_COLLAPSE_SIDEBAR_PX,
  }
}
