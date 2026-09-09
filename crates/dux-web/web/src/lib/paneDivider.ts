// One divider mechanism for both of the workspace's draggable splits: the
// sidebar's right edge (dux's own, since its width is a CSS variable rather than
// a panel) and the Changes pane's `react-resizable-panels` separator.
//
// Everything the two share lives here: the grab band, the chrome, the keyboard
// vocabulary and the persistence keys. The numbers below come from
// react-resizable-panels and are handed back to it in components/ui/resizable.tsx.

// The smallest a divider's grab band may be, per pointer kind: the library's own
// `resizeTargetMinimumSize` defaults, passed straight back by `ResizablePanelGroup`.
export const DIVIDER_TARGET_MIN = { coarse: 20, fine: 10 } as const

// The transparent grab band, painted as a centred ::after so the visible line
// stays hair-thin. Widths must match DIVIDER_TARGET_MIN; a unit test pins that.
// Tailwind scans source text for literal class names, so these cannot be built
// from the constants above at runtime.
//
// The band deliberately overlaps its neighbours' controls by a few pixels: a
// stray press on one is cheap to undo, while a band too narrow for a finger
// leaves the split immovable on a touch screen.
export const DIVIDER_HIT_SLOP =
  "after:absolute after:inset-y-0 after:left-1/2 after:w-[10px] after:-translate-x-1/2 pointer-coarse:after:w-[20px]"

// Says a finger is on this divider right now. dux's own attribute on both
// dividers, written by the hooks in hooks/use-divider-drag.ts, deliberately not
// the library's `data-separator`: the library hears no `pointercancel`, so its
// separator stays latched active after the browser takes a touch away.
//
// It exists because `hover:` is unreachable with a finger, and it is always
// present, written `false` when a divider is wired up, so both dividers can be
// asked the same question.
export const DIVIDER_HELD_ATTR = "data-dux-held"
export const DIVIDER_HELD_ON = "true"
export const DIVIDER_HELD_OFF = "false"

// While the sidebar's edge is held its width must not animate: the primitive's
// width tween restarts on every pointer move, so the edge would trail the finger
// and keep moving after it lifts. The collapse toggle keeps its tween.
//
// Written on the wrapper that carries `--sidebar-width`, since the animated
// elements are the primitive's internals, and present only during a gesture.
export const SIDEBAR_RESIZING_ATTR = "data-dux-sidebar-resizing"

// The suppression itself, worn by every element whose width the sidebar animates.
// Tailwind scans source text for literal class names, so it cannot be built here.
export const SIDEBAR_RESIZING_NO_TRANSITION =
  "[[data-dux-sidebar-resizing]_&]:transition-none"

// The held tone: deliberately the same `bg-ring` the hover rule paints, so the two
// cannot disagree whichever order Tailwind emits them in.
export const DIVIDER_ACTIVE_PAINT = "data-[dux-held=true]:bg-ring"

// Stacking, worn by both dividers. A divider is a sibling of the panes it splits,
// so a later-painted pane covers its grab band; a press then lands on the pane,
// whose `touch-action` is `auto`, and the browser claims the gesture as a scroll.
export const DIVIDER_STACKING = "z-30"

// Everything a divider element wears whatever side it is on. Positioning is
// deliberately not here: each site declares its own, since the Changes divider is
// a flex item and the sidebar's is pinned to the sidebar's edge.
//
// `touch-none` is load-bearing: without it the browser claims a finger's drag as a
// page pan and answers with `pointercancel`, which drag handlers read as drag-end.
export const DIVIDER_CHROME =
  "w-px bg-border hover:bg-ring " +
  DIVIDER_ACTIVE_PAINT +
  " " +
  DIVIDER_STACKING +
  // The focus ring is `focus-visible:` and nothing else: a press moves focus to the
  // divider so a drag can continue from the keyboard, and a ring painted for that
  // press would stand under a finger that has already lifted.
  " cursor-col-resize touch-none focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden " +
  DIVIDER_HIT_SLOP

// Where a press counts as a press on the divider, decided from the element's rect
// rather than a DOM hit test: the band can be wider than the element, and a press
// inside it belongs to the divider even under something painted on top.
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

// Whether a press belongs to a divider, by the one rule both dividers use. Two
// ways qualify and both are needed: the band (the rect test above), which decides
// every press the browser gave to a neighbour, and the browser's own target, since
// a touch adjusted onto this divider can arrive with coordinates outside the band.
export function dividerPressHits(
  el: HTMLElement | null,
  event: { target: EventTarget | null; clientX: number; clientY: number },
  minWidth: number,
): boolean {
  if (el === null) return false
  const target = event.target
  if (target === el || (target instanceof Node && el.contains(target))) {
    return true
  }
  const band = dividerHitBand(el.getBoundingClientRect(), minWidth)
  return withinDividerBand(band, event.clientX, event.clientY)
}

// The keyboard vocabulary of a vertical divider, matching the library's own
// separator keydown handler: arrows step, Home and End run it to its ends, Enter
// toggles the collapse of the collapsible side. The action says which way and how
// far in kind, never in pixels: the two dividers step by different amounts.
export type DividerKeyAction =
  | { kind: "step"; direction: -1 | 1; toEnd: boolean }
  | { kind: "toggle" }

export function dividerKeyAction(key: string): DividerKeyAction | null {
  switch (key) {
    case "ArrowLeft":
      return { kind: "step", direction: -1, toEnd: false }
    case "ArrowRight":
      return { kind: "step", direction: 1, toEnd: false }
    case "Home":
      return { kind: "step", direction: -1, toEnd: true }
    case "End":
      return { kind: "step", direction: 1, toEnd: true }
    case "Enter":
      return { kind: "toggle" }
    default:
      return null
  }
}

// The cursor the library paints over the whole document while a divider is hovered
// or dragged: `ew-resize` where it renders directionally, `col-resize` elsewhere.
export function dividerCursor(userAgent: string): string {
  return userAgent.includes("Chrome") || userAgent.includes("Firefox")
    ? "ew-resize"
    : "col-resize"
}

// How far a pointer must travel before a gesture counts as a drag rather than a
// press that went nowhere, so a press alone commits nothing: a cancelled touch can
// otherwise reach the library as a full-scale delta that collapses the pane.
// Three pixels rather than zero because a finger resting on glass jitters.
export const DIVIDER_DRAG_THRESHOLD_PX = 3

// Where each divider's released size is remembered: localStorage entries written
// at the end of a gesture, never during one and never from a mount.
export const DIVIDER_STORAGE_KEYS = {
  sidebarWidth: "dux:sidebar-width",
  changesPanePercent: "dux:changes-pane-percent",
} as const

// Every localStorage touch here is guarded: private mode, blocked site data or a
// full quota throws on read and write, and losing the remembered width is the
// whole cost, never a failed render or a drag that throws on release.
export function readStoredText(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

export function writeStoredText(key: string, value: string): void {
  try {
    localStorage.setItem(key, value)
  } catch {
    // Nothing to do and nothing to say: the size is still applied, it just
    // will not survive a reload.
  }
}

export function readStoredPanePercent(
  key: string,
  fallback: number,
  min: number,
  max: number,
): number {
  const raw = readStoredText(key)
  if (raw === null) return fallback
  const parsed = Number.parseFloat(raw)
  // Anything unreadable or outside the band falls back to the default rather than
  // being clamped into it, so a half-written entry cannot strand a pane at nothing.
  if (!Number.isFinite(parsed) || parsed < min || parsed > max) return fallback
  return parsed
}

export function writeStoredPanePercent(key: string, percent: number): void {
  writeStoredText(key, String(percent))
}
