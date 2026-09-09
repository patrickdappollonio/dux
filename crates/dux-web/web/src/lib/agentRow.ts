import type { SessionStatus } from "@/lib/types"

/** Visual treatment for an agent row, shared by the desktop sidebar and the
 *  mobile shell so the two surfaces never drift.
 *
 *  - `shimmer`: working. Drives the name shimmer and the Bot icon's bob
 *    together, so the two motion cues stay in lockstep. Requires `active`.
 *  - `dimmed`: not running, so the whole row recedes and running agents stand
 *    out. Mutually exclusive with `shimmer` by construction.
 *  - `attention`: a tab wants the user (a permission prompt, a finished turn),
 *    shown as a cyan dot. Orthogonal to the other flags.
 *  - `typing`: streaming keystroke-level input, shown as the caret and the
 *    violet "Typing" word. Requires `active`, and suppresses `shimmer` so a row
 *    shows the caret or the bob, never both. */
export function agentRowVisual(
  status: SessionStatus,
  working: boolean,
  needsAttention = false,
  typing = false,
): { shimmer: boolean; dimmed: boolean; attention: boolean; typing: boolean } {
  const active = status === "active"
  const isTyping = active && typing
  return {
    // Working cue is suppressed during typing so the two states never fire at
    // once; the caret carries "typing", the bob/shimmer carry "working".
    shimmer: active && working && !isTyping,
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
