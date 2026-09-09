// The floating pill's geometry and gesture, as pure rules: where a rectangle may
// sit inside another, when a press stops being a tap, and what a stored position
// must look like to be believed. None of it is rendering.
//
// The position is remembered per device under one key, not per pane: a corner
// the user cleared is a fact about how they hold the device rather than about
// which agent they were looking at. It is a viewer convenience, so it lives in
// `localStorage` and every access degrades quietly.

import { MOUSE_DRAG_ACTIVATION, TOUCH_DRAG_ACTIVATION } from "./dragActivation"

/** One `localStorage` key for the whole device. */
export const THEATER_PILL_POSITION_KEY = "dux:theater-pill-position"

/** The latch behind the once-per-device "hold the grip" hint. */
export const THEATER_PILL_HINT_KEY = "dux:theater-pill-hint"

/// How far the default resting place sits off the surface's edges, in pixels.
/// Equal to Tailwind's `3.5` spacing step (0.875rem at the 16px root).
export const THEATER_PILL_MARGIN = 14

/// The grip's own width, a per-axis relaxation of the 40px floor argued at the
/// button itself. It must stay equal to the button's class, which a test pins.
export const THEATER_PILL_GRIP_W_PX = 18

/// The gap between the pill's controls, Tailwind's `gap-0.5`.
export const THEATER_PILL_ROW_GAP_PX = 2

/// The width the cluster gains leaving the flap and gives back on the way home:
/// the grip plus its gap. The docked flap reserves no space for it, so anything
/// measuring the pill mid-flight for a resting place must add it back.
export const THEATER_PILL_GRIP_SLOT_PX =
  THEATER_PILL_GRIP_W_PX + THEATER_PILL_ROW_GAP_PX

/// The class the collapsed slot is expressed by, shared by the component that
/// applies it and the measurement that has to know it is applied.
export const PILL_GRIPLESS_CLASS = "dux-pill-gripless"

/// How far one arrow-key press moves the pill. Nudging is the keyboard's only
/// way to clear an occluded corner, so the step is small enough to place the
/// pill precisely and large enough to cross a pane on a held key.
export const THEATER_PILL_NUDGE_PX = 16

/// How far a finger has to slide on the grip before the pill lifts. No hold
/// gates it, unlike the sidebar's reorder drag: the grip is nothing but a drag
/// handle, so only tap versus drag is left to decide. Wider than the mouse's
/// slop because a finger wobbles on contact.
export const THEATER_PILL_TOUCH_DISTANCE = TOUCH_DRAG_ACTIVATION.tolerance

/// How far a mouse has to pull before the press becomes a drag. A plain click
/// stays a click, exactly like the reorder drags.
export const THEATER_PILL_MOUSE_DISTANCE = MOUSE_DRAG_ACTIVATION.distance

/** A point in the surface's own coordinate space: the pill's top-left corner. */
export interface PillPosition {
  x: number
  y: number
}

/** Just the two numbers a clamp needs from a rectangle. */
export interface PillSize {
  width: number
  height: number
}

function clampAxis(value: number, span: number): number {
  // A surface too small to hold the pill has no room to give, so the pill goes
  // to the origin and overhangs rather than being pushed off the near edge too.
  if (span <= 0) return 0
  return Math.min(Math.max(value, 0), span)
}

/**
 * Keep the pill wholly inside the surface. Every mover (the drag, the arrow
 * keys, the restore, the resize observer) ends here, so whatever moved the pill
 * leaves every one of its buttons reachable.
 */
export function clampPillPosition(
  pos: PillPosition,
  surface: PillSize,
  pill: PillSize,
): PillPosition {
  return {
    x: clampAxis(pos.x, surface.width - pill.width),
    y: clampAxis(pos.y, surface.height - pill.height),
  }
}

/**
 * Where the pill sits before anybody has moved it: the bottom-right corner, the
 * thumb's on a held device and the one an agent CLI is least likely to be
 * drawing something that must be read in.
 */
export function defaultPillPosition(
  surface: PillSize,
  pill: PillSize,
): PillPosition {
  return clampPillPosition(
    {
      x: surface.width - pill.width - THEATER_PILL_MARGIN,
      y: surface.height - pill.height - THEATER_PILL_MARGIN,
    },
    surface,
    pill,
  )
}

/**
 * The position a freshly measured pill takes: the remembered one where there is
 * one, the default corner otherwise, and either way inside today's surface.
 *
 * `null` when the surface has no size yet: a pane that has not been laid out
 * cannot be clamped into honestly, so the caller keeps its CSS default rather
 * than jumping the pill to the origin for a frame.
 */
export function resolvePillPosition(
  stored: PillPosition | null,
  surface: PillSize,
  pill: PillSize,
): PillPosition | null {
  if (surface.width <= 0 || surface.height <= 0) return null
  return stored
    ? clampPillPosition(stored, surface, pill)
    : defaultPillPosition(surface, pill)
}

/**
 * One arrow-key press, or `null` for any other key. `null` rather than the
 * unchanged position, so the caller can tell what to swallow: every other key
 * on a focused grip still belongs to the page.
 */
export function nudgePillPosition(
  pos: PillPosition,
  key: string,
  surface: PillSize,
  pill: PillSize,
): PillPosition | null {
  const step = THEATER_PILL_NUDGE_PX
  const moved =
    key === "ArrowLeft"
      ? { x: pos.x - step, y: pos.y }
      : key === "ArrowRight"
        ? { x: pos.x + step, y: pos.y }
        : key === "ArrowUp"
          ? { x: pos.x, y: pos.y - step }
          : key === "ArrowDown"
            ? { x: pos.x, y: pos.y + step }
            : null
  return moved ? clampPillPosition(moved, surface, pill) : null
}

/** What one press on the grip has turned out to be, so far. */
export type PillGestureVerdict = "pending" | "lift" | "tap"

export interface PillGestureInput {
  /// The pointer event's own `pointerType`.
  pointerType: string
  /// How far the pointer has travelled from where it landed, in pixels.
  travel: number
  /// Whether the press has been released (or cancelled by the browser).
  ended: boolean
}

/**
 * Tap or drag? One gate for both pointer kinds, and it is travel: the pill lifts
 * once the pointer passes the slop, and the pointer kind only chooses how much
 * slop that is. Nothing waits on a clock, because the grip is a dedicated drag
 * handle with no second gesture to disambiguate from. A press that ends first is
 * a plain tap and does nothing.
 */
export function classifyPillGesture(input: PillGestureInput): PillGestureVerdict {
  if (input.ended) return "tap"
  const slop =
    input.pointerType === "touch"
      ? THEATER_PILL_TOUCH_DISTANCE
      : THEATER_PILL_MOUSE_DISTANCE
  return input.travel >= slop ? "lift" : "pending"
}

// Storage can be absent or throw outright (private mode, a browser set to block
// site data). Both degrade to "nothing remembered".
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage
  } catch {
    return null
  }
}

/// Nothing legitimate is this far into a surface; anything beyond it is a
/// corrupted or hand-edited value rather than a position this app wrote.
const MAX_STORED_COORDINATE = 100_000

function validCoordinate(value: unknown): value is number {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    value >= 0 &&
    value <= MAX_STORED_COORDINATE
  )
}

/**
 * Read a stored position out of its JSON, refusing anything that is not one.
 * Everything unbelievable falls back to the default corner rather than being
 * clamped: clamping a lie still puts the pill somewhere the user never put it.
 */
export function parsePillPosition(raw: string | null): PillPosition | null {
  if (!raw) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return null
  }
  const { x, y } = parsed as { x?: unknown; y?: unknown }
  if (!validCoordinate(x) || !validCoordinate(y)) return null
  return { x, y }
}

/** The device's remembered pill position, or `null` for anything unreadable. */
export function readPillPosition(): PillPosition | null {
  try {
    return parsePillPosition(storage()?.getItem(THEATER_PILL_POSITION_KEY) ?? null)
  } catch {
    return null
  }
}

/** Remember where the user put the pill. Best-effort, whole pixels. */
export function writePillPosition(pos: PillPosition): void {
  try {
    storage()?.setItem(
      THEATER_PILL_POSITION_KEY,
      JSON.stringify({ x: Math.round(pos.x), y: Math.round(pos.y) }),
    )
  } catch {
    // Storage refused. The pill still moves for the life of the page; only the
    // memory is lost, which is the cheap half.
  }
}

/**
 * Should the "hold the grip" hint fire on this device? A storage that cannot
 * answer reads as "already shown": without the latch the hint would fire on
 * every entry into theater, and nagging is the worse failure.
 */
export function readPillHintPending(): boolean {
  try {
    const store = storage()
    if (!store) return false
    return store.getItem(THEATER_PILL_HINT_KEY) === null
  } catch {
    return false
  }
}

/** Never hint on this device again. Best-effort. */
export function markPillHintShown(): void {
  try {
    storage()?.setItem(THEATER_PILL_HINT_KEY, "shown")
  } catch {
    // See `readPillHintPending`: a storage that refuses writes also refuses
    // reads, so the hint is already suppressed.
  }
}
