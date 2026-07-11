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

/** Whether a viewed ping should be sent right now: only when this device owns the
 * PTY input AND its document is visible (foregrounded). */
export function shouldSendViewed(ctx: {
  isOwner: boolean
  visible: boolean
}): boolean {
  return ctx.isOwner && ctx.visible
}
