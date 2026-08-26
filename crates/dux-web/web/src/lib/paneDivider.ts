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
// WHAT THE BAND OVERLAPS, and why that is accepted. It reaches about 10px into
// each neighbour under a finger, 5px under a mouse, and those neighbours do
// carry controls:
//
//   - Left of the sidebar's edge sit the agent rows, whose right end is live:
//     pressing one selects the agent and navigates to it.
//   - Right of the Changes divider sits the changed-files list, whose status
//     marker (which doubles as the selection checkbox) starts about 5px in.
//
// The overlap is accepted anyway, on two grounds. It is not new: the panel
// library has always claimed exactly this band for the Changes divider in the
// capture phase, so the strip was never reaching the file rows to begin with,
// and the sidebar's edge has carried a 20px coarse slop since the touch pass
// before this one. And both losses are cheap and instantly undone: selecting
// the wrong agent is one press to correct and destroys nothing, and the file
// row's checkbox has the whole rest of a 40px row to be hit in. What a
// too-narrow divider costs is worse: on a touch screen the split simply cannot
// be moved at all.
export const DIVIDER_HIT_SLOP =
  "after:absolute after:inset-y-0 after:left-1/2 after:w-[10px] after:-translate-x-1/2 pointer-coarse:after:w-[20px]"

// WHICH ATTRIBUTE SAYS "a finger is on me right now". dux's own, on both
// dividers, written by the two hooks in hooks/use-divider-drag.ts.
//
// It is deliberately NOT react-resizable-panels' `data-separator`, which was
// what lit the Changes divider before. Measured on a touch tablet against
// 4.11.2: the library has no `pointercancel` listener, so a touch the browser
// takes away leaves its separator latched at `data-separator="active"` for
// good, and the held paint stayed lit with no finger anywhere near the glass.
// The library's attribute is a report about the library's own gesture state;
// what the paint needs is a report about the USER's, and dux is the only one
// of the two that hears a cancelled pointer.
//
// The held paint exists because `hover:` is not reachable with a finger: a held
// divider looked identical to an idle one on a touch screen, and the only way
// to tell whether the gesture had been acquired was to watch the pane move.
//
// Always present, never removed: it is written `false` as soon as a divider is
// wired up, so "is this element driven by dux's held paint at all" is a
// question the parity test can ask of both dividers.
export const DIVIDER_HELD_ATTR = "data-dux-held"
export const DIVIDER_HELD_ON = "true"
export const DIVIDER_HELD_OFF = "false"

// The held tone. Deliberately the same `bg-ring` the hover rule paints, so the
// two rules cannot disagree whichever order Tailwind emits them in: a mouse
// hovering and a finger holding mean the same thing to the eye.
export const DIVIDER_ACTIVE_PAINT = "data-[dux-held=true]:bg-ring"

// STACKING. A divider sits between two panes and is a sibling of both, so
// whichever pane is painted later covers it: not just the hair-thin line, but
// the transparent grab band, which is what a press has to land on. When the
// band is covered, a finger's press targets the PANE instead, the pane's
// `touch-action` is `auto`, the browser claims the gesture as a scroll and
// answers with `pointercancel` before the divider has moved a pixel.
//
// The sidebar's edge has always carried this; the Changes separator did not,
// and that is exactly the difference a finger could feel. Both wear it now.
export const DIVIDER_STACKING = "z-30"

// Everything a divider element wears whatever side it is on: the hair-thin
// painted line, its hover tone, the resize cursor, the focus ring and the grab
// band.
//
// `touch-none` is load-bearing: without it the browser claims a finger's
// horizontal drag as a page pan and answers with `pointercancel`, which every
// drag handler (correctly) reads as drag-end, so the divider never moves under
// a finger. The library hard-codes the same `touch-action: none` inline on its
// Separator for the same reason (react-resizable-panels issue 662); this covers
// the whole grab band rather than only the painted line, because a
// pseudo-element's touch-action is its originating element's.
//
// Positioning is deliberately NOT in here: the Changes divider is a flex item
// and the sidebar's is pinned to the sidebar's edge, so each site declares its
// own. Everything else about how a divider looks and behaves is shared.
export const DIVIDER_CHROME =
  "w-px bg-border hover:bg-ring " +
  DIVIDER_ACTIVE_PAINT +
  " " +
  DIVIDER_STACKING +
  // The focus ring is `focus-visible:` and nothing else: a press MOVES focus to
  // the divider (both hooks do it, so a drag can be continued from the
  // keyboard), and a ring painted for that press is a ring left standing under
  // a finger that has already lifted. Both hooks focus with `focusVisible:
  // false` for the same reason; see use-divider-drag.ts.
  " cursor-col-resize touch-none focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden " +
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

// Whether a press belongs to a divider element, by the one rule both dividers
// use.
//
// TWO WAYS TO QUALIFY, and both are needed.
//
// The BAND is the rect test above, which is what react-resizable-panels does:
// a press inside the band belongs to the divider even when something else is
// painted on top of it, and the band can be wider than the element without the
// element having to grow.
//
// The TARGET is the browser's own verdict. A browser adjusts a touch point
// before it dispatches: Chrome grows a finger's contact area into a rectangle
// and picks the most plausible target inside it, which reaches about 20px on
// either side of a thin control. So a press the browser has ALREADY decided
// belongs to this divider can arrive with coordinates outside the divider's own
// band, and the rect test alone would refuse a gesture the platform had handed
// over. The strip between the two widths was dead: it neither resized nor
// scrolled.
//
// The band still decides every press the browser gave to a NEIGHBOUR, which is
// the case it exists for: something painted over the divider must not be able
// to swallow the gesture.
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

// The keyboard vocabulary of a vertical divider, copied from the library's own
// separator keydown handler: the arrows step, Home and End run the divider to
// its ends, and Enter toggles the collapse of the collapsible side.
//
// The action says WHICH WAY and HOW FAR IN KIND, never how many pixels: the
// library steps by 5% of the group it splits, and the sidebar deliberately does
// not (see SidebarDragEdge, and the accepted-differences list in the plan).
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

// The cursor the library paints over the whole document while a divider is
// hovered or dragged. Chrome and Firefox render the directional `ew-resize`
// glyph; everywhere else `col-resize` is the one that reads as a splitter.
export function dividerCursor(userAgent: string): string {
  return userAgent.includes("Chrome") || userAgent.includes("Firefox")
    ? "ew-resize"
    : "col-resize"
}

// How far a pointer must travel before a gesture counts as a DRAG rather than
// a press that landed and went nowhere.
//
// A press alone must never commit anything. react-resizable-panels 4.11.2 has
// no `pointercancel` listener, so a touch the browser takes away leaves its
// separator latched "active"; the `pointerleave` that Chrome fires next is then
// handled as a move with no press point, and its own code reads that as a
// full-scale delta (`clientX < 0 ? -100 : 100`), which drives the pane straight
// to its collapsed size. Requiring real travel means that layout report is
// recognised for what it is and undone, instead of being written to the server
// as "the user closed the Changes pane".
//
// Three pixels rather than zero because a finger resting on glass jitters.
export const DIVIDER_DRAG_THRESHOLD_PX = 3

// Where each divider's released size is remembered. Both are plain localStorage
// entries written at the END of a gesture, never during one and never from a
// mount.
export const DIVIDER_STORAGE_KEYS = {
  sidebarWidth: "dux:sidebar-width",
  changesPanePercent: "dux:changes-pane-percent",
} as const

// EVERY localStorage touch in here is guarded. A browser in private mode, with
// site data blocked, or over its quota throws on both read and write, and a
// divider that cannot remember its width must still be a divider: losing the
// memory is the whole cost, never a screen that fails to render or a drag that
// throws on release.
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
  // A hand-edited or half-written entry must not strand a pane at nothing, or
  // squeeze its neighbour under the neighbour's own minimum: anything
  // unreadable or outside the band falls back to the default rather than being
  // clamped into it.
  if (!Number.isFinite(parsed) || parsed < min || parsed > max) return fallback
  return parsed
}

export function writeStoredPanePercent(key: string, percent: number): void {
  writeStoredText(key, String(percent))
}
