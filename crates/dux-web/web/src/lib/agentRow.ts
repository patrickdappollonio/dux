import type { SessionStatus } from "@/lib/types"

/** Visual treatment for an agent row, shared by the desktop sidebar and the
 *  mobile shell so the two surfaces never drift.
 *
 *  - `shimmer`: the agent is actively streaming output → its name gets the
 *    shimmer. The same flag drives the Bot icon's bob, so the two "working"
 *    motion cues stay in lockstep.
 *  - `dimmed`: the agent isn't running (detached/exited) → the whole row recedes
 *    (name, icon, and status indicator) so the running agents stand out. Mirrors
 *    the dux TUI, where active sessions render brighter than detached/exited.
 *
 *  The two are mutually exclusive by construction: `shimmer` requires `active`,
 *  `dimmed` requires not-`active`.
 *
 *  - `attention`: any of the agent's tabs needs attention (a permission prompt,
 *    a finished turn) the user has not looked at → an amber dot on the row. The
 *    server tears the flag down when the tab exits, so it is effectively only
 *    ever set on a live (active) agent; it is orthogonal to shimmer/dimmed (a
 *    flagged agent may still be streaming its prompt). */
export function agentRowVisual(
  status: SessionStatus,
  working: boolean,
  needsAttention = false,
): { shimmer: boolean; dimmed: boolean; attention: boolean } {
  return {
    shimmer: status === "active" && working,
    dimmed: status !== "active",
    attention: needsAttention,
  }
}
