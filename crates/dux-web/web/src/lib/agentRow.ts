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
 *    a finished turn) the user has not looked at → a cyan dot on the row. The
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

// The status dot color, mirroring StatusBadge's STATUS map (active=green,
// detached=amber, exited=muted). Kept here (a framework-free lib file, not a
// component file) so both StatusBadge and any other surface building its own
// status line (the agent vitals tooltip) can share the exact mapping without
// re-deriving it and risking drift, and so StatusBadge.tsx stays a
// components-only export for React Fast Refresh. `needsAttention` takes
// precedence over the raw status, matching the cyan "needs attention"
// treatment used elsewhere (the sidebar row's Bot icon, the favicon dot).
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
