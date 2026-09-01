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

/// How far one arrow-key press moves the pill.
///
/// Keyboard nudging exists because dragging is a pointer gesture and a keyboard
/// user would otherwise have no way to clear an occluded corner at all. The step
/// is small enough to place the pill precisely and large enough that crossing a
/// pane takes a held key rather than a hundred presses.
export const THEATER_PILL_NUDGE_PX = 16

/// A finger has to hold this long on the grip before the pill lifts.
/// The same hold the sidebar's reorder drag uses, for the same reasons written
/// down there: below it a scroll-intent touch arms the drag, above it the
/// browser's own long-press behaviors start competing.
export const THEATER_PILL_HOLD_MS = TOUCH_DRAG_ACTIVATION.delay

/// How far a finger may slide during the hold before the gesture is read as a
/// scroll and abandoned.
export const THEATER_PILL_TOUCH_TOLERANCE = TOUCH_DRAG_ACTIVATION.tolerance

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
export type PillGestureVerdict = "pending" | "lift" | "cancel" | "tap"

export interface PillGestureInput {
  /// The pointer event's own `pointerType`.
  pointerType: string
  /// How long the press has been held, in milliseconds.
  heldMs: number
  /// How far the pointer has travelled from where it landed, in pixels.
  travel: number
  /// Whether the press has been released (or cancelled by the browser).
  ended: boolean
}

/**
 * Tap or drag?
 *
 * The two pointer kinds need OPPOSITE gates, the same split the reorder drags
 * are built on. A MOUSE lifts on travel, because a mouse can be held still and
 * a click must stay instant. A FINGER lifts on time, because a finger cannot be
 * held still and every drag would otherwise start on contact and fight the
 * gesture underneath it; sliding away before the hold completes is a scroll,
 * not a slow drag, so it cancels outright rather than waiting for the timer.
 *
 * A press that ends before either gate is a plain tap, which is what keeps the
 * grip harmless: it does nothing, and the buttons beside it keep their meanings.
 */
export function classifyPillGesture(input: PillGestureInput): PillGestureVerdict {
  if (input.ended) return "tap"
  if (input.pointerType === "touch") {
    if (input.travel > THEATER_PILL_TOUCH_TOLERANCE) return "cancel"
    return input.heldMs >= THEATER_PILL_HOLD_MS ? "lift" : "pending"
  }
  return input.travel >= THEATER_PILL_MOUSE_DISTANCE ? "lift" : "pending"
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
