// One divider mechanism for both of the workspace's draggable splits: the
// sidebar's right edge and the Changes pane's separator.
//
// The right-hand divider is `react-resizable-panels`' Separator; the sidebar's
// is dux's own, because the sidebar's width is a CSS variable on
// SidebarProvider rather than a panel in a layout group. Two implementations of
// the same gesture drifted, and a finger could work one and not the other. So
// everything the two can share lives here: the grab band, the chrome, the
// keyboard vocabulary and the persistence keys. The numbers below are read from
// react-resizable-panels 4.11.2 and handed BACK to it (see
// components/ui/resizable.tsx), so neither side can move without the other.

// The smallest a divider's grab band may be, per pointer kind. These are the
// library's own `resizeTargetMinimumSize` defaults; `ResizablePanelGroup`
// passes this object straight back to it.
export const DIVIDER_TARGET_MIN = { coarse: 20, fine: 10 } as const

// The transparent grab band, painted as a centred ::after so the visible line
// stays hair-thin. Widths must match DIVIDER_TARGET_MIN; a unit test pins that.
//
// Tailwind scans source text for literal class names, so these cannot be built
// from the constants above at runtime.
//
// The band overlaps its neighbours by half its width. That is the per-axis
// justification the touch-target tenet asks for: on both dividers the
// overlapped strip belongs to a scrollable list or a terminal edge that carries
// no control of its own, and the acquisition below already claimed that band
// before the neighbour could see the press. The band is horizontal-only; both
// of dux's dividers are vertical lines.
export const DIVIDER_HIT_SLOP =
  "after:absolute after:inset-y-0 after:left-1/2 after:w-[10px] after:-translate-x-1/2 pointer-coarse:after:w-[20px]"

// Everything a divider element wears whatever side it is on. `touch-none` is
// load-bearing: without it the browser claims a finger's horizontal drag as a
// page pan and answers with `pointercancel`, which every drag handler
// (correctly) reads as drag-end, so the divider never moves under a finger.
// The library hard-codes the same `touch-action: none` inline on its Separator
// for the same reason (react-resizable-panels issue 662); this covers the whole
// grab band rather than only the painted line, because a pseudo-element's
// touch-action is its originating element's.
// Positioning is deliberately NOT in here: the Changes divider is a flex item
// and the sidebar's is pinned to the sidebar's edge, so each site declares its
// own. Everything else about how a divider behaves is shared.
export const DIVIDER_CHROME =
  "cursor-col-resize touch-none focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden " +
  DIVIDER_HIT_SLOP

// Where a press counts as a press on the divider.
//
// Both sides decide this from the element's RECT rather than from a DOM hit
// test, which is what the library does: a press inside the band belongs to the
// divider even when something else is painted on top of it, and the band can be
// wider than the element without the element having to grow.
export interface DividerBand {
  left: number
  right: number
  top: number
  bottom: number
}

export function dividerHitBand(
  rect: { left: number; right: number; top: number; bottom: number },
  minWidth: number,
): DividerBand {
  const width = rect.right - rect.left
  const grow = width < minWidth ? (minWidth - width) / 2 : 0
  return {
    left: rect.left - grow,
    right: rect.right + grow,
    top: rect.top,
    bottom: rect.bottom,
  }
}

export function withinDividerBand(
  band: DividerBand,
  x: number,
  y: number,
): boolean {
  return x >= band.left && x <= band.right && y >= band.top && y <= band.bottom
}

// The keyboard vocabulary of a vertical divider, copied from the library's own
// separator keydown handler: arrows step by 5, Home and End run the divider to
// its ends, Enter toggles the collapse of the collapsible side. Steps are
// PERCENTAGES OF THE GROUP the divider splits, which for both of dux's
// dividers is the window's width.
export type DividerKeyAction =
  | { kind: "step"; percent: number }
  | { kind: "toggle" }

export const DIVIDER_KEY_STEP_PERCENT = 5

export function dividerKeyAction(key: string): DividerKeyAction | null {
  switch (key) {
    case "ArrowLeft":
      return { kind: "step", percent: -DIVIDER_KEY_STEP_PERCENT }
    case "ArrowRight":
      return { kind: "step", percent: DIVIDER_KEY_STEP_PERCENT }
    case "Home":
      return { kind: "step", percent: -100 }
    case "End":
      return { kind: "step", percent: 100 }
    case "Enter":
      return { kind: "toggle" }
    default:
      return null
  }
}

// The cursor the library paints over the whole document while a divider is
// hovered or dragged. Chrome and Firefox render the directional `ew-resize`
// glyph; everywhere else `col-resize` is the one that reads as a splitter.
export function dividerCursor(userAgent: string): string {
  return userAgent.includes("Chrome") || userAgent.includes("Firefox")
    ? "ew-resize"
    : "col-resize"
}

// Where each divider's released size is remembered. Both are plain localStorage
// entries written at the END of a gesture, never during it.
export const DIVIDER_STORAGE_KEYS = {
  sidebarWidth: "dux:sidebar-width",
  changesPanePercent: "dux:changes-pane-percent",
} as const

export function readStoredPanePercent(
  key: string,
  fallback: number,
  min: number,
): number {
  if (typeof localStorage === "undefined") return fallback
  const raw = localStorage.getItem(key)
  if (raw === null) return fallback
  const parsed = Number.parseFloat(raw)
  // A hand-edited or half-written entry must not strand the pane at nothing:
  // anything unreadable, out of range, or below the width that still shows the
  // pane falls back to the default rather than being clamped up to it.
  if (!Number.isFinite(parsed) || parsed < min || parsed > 100) return fallback
  return parsed
}

export function writeStoredPanePercent(key: string, percent: number): void {
  if (typeof localStorage === "undefined") return
  localStorage.setItem(key, String(percent))
}
