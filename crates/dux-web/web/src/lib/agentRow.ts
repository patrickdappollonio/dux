import type { SessionStatus } from "@/lib/types"

/** Visual treatment for an agent row, shared by the desktop sidebar and the
 *  mobile shell so the two surfaces never drift.
 *
 *  - `working`: busy, AND nothing outranks it. Drives the glyph pulse and the
 *    state word's own pulse together, so the two halves of the one cue stay in
 *    lockstep. It follows the same priority ladder the state word does, so it
 *    is on exactly when the word reads "Working"; a test walks the whole input
 *    space to keep the two from parting.
 *  - `dimmed`: not running, so the whole row recedes and running agents stand
 *    out. Mutually exclusive with `working` by construction.
 *  - `attention`: a tab wants the user (a permission prompt, a finished turn),
 *    shown as a cyan dot. Orthogonal to the other flags, and it OUTRANKS
 *    working.
 *  - `typing`: streaming keystroke-level input, shown as the caret and the
 *    violet "Typing" word. Requires `active`, and suppresses `working` so a row
 *    shows the caret or the pulse, never both. */
export function agentRowVisual(
  status: SessionStatus,
  working: boolean,
  needsAttention = false,
  typing = false,
): { working: boolean; dimmed: boolean; attention: boolean; typing: boolean } {
  const active = status === "active"
  const isTyping = active && typing
  return {
    // The working cue stands down for both states above it, so the row shows
    // one cue at a time: the caret carries "typing", the cyan blink carries
    // "needs you", and the pulse carries "working". Attention matters twice
    // over here, because its blink lives on the wrapper the pulsing glyph sits
    // inside, and two opacity animations stacked that way MULTIPLY into a dip
    // far deeper than either one asks for.
    working: active && working && !isTyping && !needsAttention,
    dimmed: !active,
    attention: needsAttention,
    typing: isTyping,
  }
}

// The status dot color, shared by StatusBadge and any other surface building a
// status line so the mapping cannot drift. It lives in a framework-free lib
// file rather than beside StatusBadge, which stays a components-only export for
// React Fast Refresh. `needsAttention` outranks the raw status, matching the
// cyan treatment the Bot icon and the favicon dot use.
const STATUS_DOT_COLOR: Record<SessionStatus, string> = {
  active: "text-green-500",
  detached: "text-amber-500",
  exited: "text-muted-foreground",
}

export function statusDotColorClass(
  status: SessionStatus,
  needsAttention = false,
): string {
  return needsAttention ? "text-cyan-100" : STATUS_DOT_COLOR[status]
}
