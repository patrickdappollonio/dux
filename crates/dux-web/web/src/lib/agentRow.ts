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
 *  `dimmed` requires not-`active`. */
export function agentRowVisual(
  status: SessionStatus,
  working: boolean,
): { shimmer: boolean; dimmed: boolean } {
  return {
    shimmer: status === "active" && working,
    dimmed: status !== "active",
  }
}
