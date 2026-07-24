// Pure decision helper for the terminal's "user is looking at this tab" ping.
//
// The terminal pane sends a lightweight viewed ping to the server (every ~2s,
// and immediately on gaining ownership or on becoming visible) so an agent the
// user is actively watching keeps its "needs attention" flag down without
// requiring keystrokes. This mirrors the TUI, which stamps the focused agent tab
// every tick. The ping is only meaningful when this device is BOTH the input
// owner (a read-only observer must not suppress attention for everyone on the
// shared engine) AND foregrounded (a backgrounded browser tab keeps its PTY
// socket open, so open-ness alone must never be taken as "watching").
//
// The branching lives here, as a pure function, so it is testable without
// mounting xterm or a live socket (matching the `termkeys.ts` convention).

/** How often to re-send the viewed ping while engaged, in ms. Comfortably under
 * the engine's 3s ATTENTION_ENGAGED_WINDOW so continuous viewing never lets the
 * flag rise between pings. */
export const VIEWED_PING_INTERVAL_MS = 2000

/** Fallback attention grace, in seconds, when the server omits
 * `ui.attention_grace_seconds` (older servers) or before the first bootstrap
 * fetch lands. Mirrors `dux_core::config::UiConfig::attention_grace_seconds`'s
 * default (`crates/dux-core/src/config.rs`), a plain duplicated literal, not
 * generated from the Rust default, so nothing enforces the two staying equal. */
export const DEFAULT_ATTENTION_GRACE_SECONDS = 3

/**
 * The "attention grace delay" (`ui.attention_grace_seconds`, default 3s) is
 * shared with the TUI (see `dux_core::config::UiConfig::attention_grace_seconds`
 * and `crates/dux-tui/src/focus.rs`): after the document transitions
 * hidden -> visible, viewed pings are suppressed for a grace period so the
 * attention indicator for a flagged agent stays up long enough for the
 * returning user to actually see it, instead of being dismissed the instant
 * the tab regains focus. Steady-state behavior (tab already visible) and
 * initial page load are unaffected: grace only arms on an OBSERVED
 * hidden -> visible transition. The TUI applies the same grace on its own
 * signal, the terminal window regaining focus.
 *
 * Whether `now` is still within the grace window that started at
 * `visibleSince`. `visibleSince === undefined` means no transition has been
 * observed yet (e.g. initial load), so there is no grace to apply.
 * `graceMs <= 0` disables the grace entirely (today's instant-clear
 * behavior).
 */
// TWIN of the core-owned `dux_core::focus::within_attention_grace` (the
// DECISION); pinned by shared vectors (`viewedPing.test.ts` mirrors
// `focus.rs`'s `within_attention_grace_semantics`). Keep the three cases
// identical: undefined-since -> false, grace<=0 -> false, elapsed<grace -> true.
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
 * PTY input AND its document is visible (foregrounded), AND (when grace context
 * is supplied) the document is not within its post-transition attention grace
 * window. Omitting `now`/`visibleSince`/`graceMs` preserves the pre-grace
 * behavior for existing call sites. */
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
 * Computes the new hidden->visible transition timestamp given a visibility
 * sample. Call this on every observed visibility signal (visibilitychange,
 * window focus) with the previously known visible state and "since" value.
 *
 * - `prevVisible === undefined` (no prior sample, e.g. first mount): never
 *   arms the grace, even if `nowVisible` is true, so initial page load has no
 *   grace.
 * - A real `false -> true` transition arms the grace, recording `now` as the
 *   new `visibleSince`.
 * - A redundant `true -> true` signal (e.g. a `focus` event while already
 *   visible) does NOT re-arm; it returns the existing `prevSince` unchanged.
 * - Going hidden (`nowVisible === false`) always resets to `undefined`.
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
