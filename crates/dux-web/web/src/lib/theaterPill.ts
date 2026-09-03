// THE FLOATING PILL'S GEOMETRY AND GESTURE, as pure rules.
//
// The pill is the only chrome theater leaves on screen, so it is also the only
// thing that can cover the newest line of output. The answer is not a cleverer
// resting place: it is letting the user move it. Everything that decision needs
// to be right lives here rather than in the component, because all of it is
// arithmetic and policy (where a rectangle may sit inside another rectangle,
// when a press stops being a tap, what a stored position has to look like to be
// believed) and none of it is rendering.
//
// The position is remembered PER DEVICE, under one key, not per pane: a corner
// the user cleared because their agent draws there is a fact about where they
// hold the tablet, not about which agent they were looking at. That makes it a
// viewer convenience like the mode memory itself, so `localStorage` is the right
// home and every access degrades quietly.

import { MOUSE_DRAG_ACTIVATION, TOUCH_DRAG_ACTIVATION } from "./dragActivation"

/** One `localStorage` key for the whole device. */
export const THEATER_PILL_POSITION_KEY = "dux:theater-pill-position"

/** The latch behind the once-per-device "hold the grip" hint. */
export const THEATER_PILL_HINT_KEY = "dux:theater-pill-hint"

/// How far the default resting place sits off the surface's edges, in pixels.
/// Equal to Tailwind's `3.5` spacing step (0.875rem at the 16px root), which is
/// the inset the pill was pinned at before it could move.
export const THEATER_PILL_MARGIN = 14

/// The phone grip's own width, a per-axis relaxation of the 40px floor argued
/// at the button itself. It is written here as well as in the button's class
/// because the resting corner is arithmetic about it; a test pins the two
/// together.
export const THEATER_PILL_GRIP_W_PX = 18

/// The gap between the pill's controls, Tailwind's `gap-0.5`.
export const THEATER_PILL_ROW_GAP_PX = 2

/// THE WIDTH THE CLUSTER GAINS ON ITS WAY OUT of the flap and gives back on the
/// way home: the grip plus the gap that appears with it. The docked flap has no
/// grip and reserves no blank space for one, so the pill starts every detach
/// this much narrower than it will settle at, and anything measuring the pill
/// for a resting place has to add it back.
export const THEATER_PILL_GRIP_SLOT_PX =
  THEATER_PILL_GRIP_W_PX + THEATER_PILL_ROW_GAP_PX

/// The class the collapsed slot is expressed by, shared by the component that
/// applies it and the measurement that has to know it is applied.
export const PILL_GRIPLESS_CLASS = "dux-pill-gripless"

/// How far one arrow-key press moves the pill.
///
/// Keyboard nudging exists because dragging is a pointer gesture and a keyboard
/// user would otherwise have no way to clear an occluded corner at all. The step
/// is small enough to place the pill precisely and large enough that crossing a
/// pane takes a held key rather than a hundred presses.
export const THEATER_PILL_NUDGE_PX = 16

/// How far a finger has to slide on the grip before the pill lifts.
///
/// There is deliberately no hold behind it, unlike the sidebar's reorder drag:
/// a row is a row first and a drag handle only on a hold, while the grip is
/// nothing but a drag handle, carries `touch-none`, and has no scroll gesture
/// underneath it to be told apart from. So the only thing left to decide is tap
/// versus drag, and a small slop decides that without a wait. It is wider than
/// the mouse's because a finger wobbles on contact.
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
 * Keep the pill wholly inside the surface.
 *
 * The one rule the drag, the arrow keys, the restore and the resize observer
 * all share: whatever moved the pill, it ends up somewhere the user can still
 * reach every one of its buttons.
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
 * Where the pill sits before anybody has moved it: the bottom-right corner.
 *
 * That corner is the thumb's on a held tablet and the one an agent CLI is least
 * likely to be drawing something that must be read, which is why it was the
 * fixed position and why it stays the default one.
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
 * `null` when the surface has no size yet. A pane that has not been laid out
 * (the frame before the first measurement, a test that never stubbed a rect)
 * cannot be clamped into honestly, and pinning the pill at the origin for that
 * frame would be a visible jump. The caller keeps its CSS default until a real
 * measurement arrives.
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
 * One arrow-key press, or `null` for a key that is not one of the four.
 *
 * Returning `null` rather than the unchanged position is what lets the caller
 * decide whether to swallow the keystroke: every other key on a focused grip
 * still belongs to the page.
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
 * Tap or drag?
 *
 * ONE GATE FOR BOTH POINTER KINDS, and it is travel: the pill lifts as soon as
 * the pointer has moved past the slop, and the only thing the pointer kind
 * changes is how much slop that is. Nothing waits on a clock, because the grip
 * is a dedicated drag handle rather than a control with a second meaning, so
 * there is no other gesture on it a hold would have to disambiguate from.
 *
 * A press that ends before the slop is crossed is a plain tap, which is what
 * keeps the grip harmless: it does nothing, and the buttons beside it keep
 * their meanings.
 */
export function classifyPillGesture(input: PillGestureInput): PillGestureVerdict {
  if (input.ended) return "tap"
  const slop =
    input.pointerType === "touch"
      ? THEATER_PILL_TOUCH_DISTANCE
      : THEATER_PILL_MOUSE_DISTANCE
  return input.travel >= slop ? "lift" : "pending"
}

// Storage can be absent (a test that never stubbed it) or throw outright
// (Safari private mode, a browser set to block site data). Neither is a reason
// for the pill to fail to render, so both degrade to "nothing remembered".
// Same shape as `theater.ts` and `typingSurface.ts`.
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
 *
 * Everything unbelievable falls back to the default corner: a missing key, junk
 * a different app wrote, the right keys with the wrong types, and coordinates no
 * surface could have produced. A clamp would silently rescue most of those, but
 * clamping a lie still moves the pill somewhere the user never put it, and the
 * corner it has always defaulted to is the better answer.
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
 * Should the "hold the grip" hint fire on this device?
 *
 * A storage that cannot answer reads as "already shown", deliberately. The
 * latch is the only thing standing between a one-time hint and a toast on every
 * single entry into theater, and nagging is the worse failure of the two.
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
