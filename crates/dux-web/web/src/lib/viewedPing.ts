// Pure decision helper for the terminal's "user is looking at this tab" ping.
//
// The terminal pane pings the server so an agent the user is actively watching
// keeps its "needs attention" flag down without requiring keystrokes. The ping is
// only meaningful when this device is both the input owner (a read-only observer
// must not suppress attention for everyone on the shared engine) and foregrounded
// (a backgrounded browser tab keeps its PTY socket open, so open-ness alone must
// never be taken as "watching").
//
// The branching lives here, as a pure function, so it is testable without
// mounting xterm or a live socket (matching the `termkeys.ts` convention).

/** How often to re-send the viewed ping while engaged, in ms. Must stay
 * comfortably under the engine's `ATTENTION_ENGAGED_WINDOW`, or continuous
 * viewing lets the flag rise between pings. */
export const VIEWED_PING_INTERVAL_MS = 2000

/** Fallback attention grace, in seconds, until the server's
 * `ui.attention_grace_seconds` lands. A duplicated literal of
 * `dux_core::config::UiConfig::attention_grace_seconds`'s default; nothing
 * enforces the two staying equal. */
export const DEFAULT_ATTENTION_GRACE_SECONDS = 3

/**
 * Whether `now` is still within the attention grace window that started at
 * `visibleSince`. After a hidden -> visible transition, viewed pings are
 * suppressed for `ui.attention_grace_seconds` so a flagged agent's indicator
 * stays up long enough for the returning user to see it; grace arms only on an
 * observed transition, so steady state and initial load are unaffected.
 * `visibleSince === undefined` means no transition has been observed, so there is
 * no grace to apply, and `graceMs <= 0` disables it entirely.
 */
// Twin of the core-owned `dux_core::focus::within_attention_grace`, pinned by
// shared vectors (`viewedPing.test.ts` mirrors `focus.rs`'s
// `within_attention_grace_semantics`). Keep the three cases identical:
// undefined-since -> false, grace<=0 -> false, elapsed<grace -> true.
export function withinAttentionGrace(
  now: number,
  visibleSince: number | undefined,
  graceMs: number,
): boolean {
  if (visibleSince === undefined) return false
  if (graceMs <= 0) return false
  return now - visibleSince < graceMs
}

/** Whether a viewed ping should be sent right now: only when this device owns the
 * PTY input and its document is visible, and, when grace context is supplied, is
 * not within its post-transition attention grace window. */
export function shouldSendViewed(ctx: {
  isOwner: boolean
  visible: boolean
  now?: number
  visibleSince?: number
  graceMs?: number
}): boolean {
  if (!ctx.isOwner || !ctx.visible) return false
  if (ctx.now === undefined || ctx.graceMs === undefined) return true
  return !withinAttentionGrace(ctx.now, ctx.visibleSince, ctx.graceMs)
}

/**
 * Computes the new hidden->visible transition timestamp from a visibility
 * sample. Call it on every observed visibility signal with the previously known
 * visible state and "since" value.
 *
 * - `prevVisible === undefined` (no prior sample) never arms the grace, so
 *   initial page load has none.
 * - A real `false -> true` transition arms it, recording `now`.
 * - A redundant `true -> true` signal does not re-arm; `prevSince` is returned.
 * - Going hidden always resets to `undefined`.
 */
export function visibleSinceAfterTransition(
  prevVisible: boolean | undefined,
  nowVisible: boolean,
  prevSince: number | undefined,
  now: number,
): number | undefined {
  if (!nowVisible) return undefined
  if (prevVisible === true) return prevSince
  if (prevVisible === false) return now
  // prevVisible === undefined: no prior sample observed, so this can't be a
  // real hidden -> visible transition.
  return undefined
}
