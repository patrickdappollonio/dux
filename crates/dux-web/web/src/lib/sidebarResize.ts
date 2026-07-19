// Pure geometry for the desktop sidebar's drag-to-resize edge. Kept out of the
// component so the thresholds are unit-testable without a live pointer drag (and
// so exporting them doesn't break the component file's react-refresh boundary).

// The expanded sidebar's resizable band.
export const MIN_SIDEBAR_PX = 14 * 16
export const MAX_SIDEBAR_PX = 28 * 16
// Below this released width the sidebar auto-collapses to the icon rail instead
// of staying uselessly narrow. It sits a bit below the width where the footer
// split-buttons start stacking (the footer's `@[18rem]` container query, i.e. a
// ~19rem sidebar), so the collapse reads as an intentional "you dragged it too
// narrow" snap rather than twitching right at the stack point.
export const AUTO_COLLAPSE_SIDEBAR_PX = 15 * 16

// Decide the outcome of a resize-handle release: clamp the pointer x to the
// allowed band and report whether that width should auto-collapse the sidebar.
export function sidebarResizeRelease(clientX: number): {
  widthRem: string
  collapse: boolean
} {
  const px = Math.min(Math.max(clientX, MIN_SIDEBAR_PX), MAX_SIDEBAR_PX)
  return {
    widthRem: `${px / 16}rem`,
    collapse: px < AUTO_COLLAPSE_SIDEBAR_PX,
  }
}
