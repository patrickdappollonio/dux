// Pure helpers backing the flat (no-project-grouping) agent list, shared by the
// desktop sidebar and the mobile hub so the two surfaces never drift. Kept free
// of React and dnd-kit so every rule here is trivially unit-testable.
//
// The flat model replaces the project -> agents tree with a single ordered list
// of agents (their companion terminals nest under them; project terminals ride
// along as their own rows). These helpers own the three data decisions that make
// that list readable: which agents are "quiet" (dormant) and collapse into a
// tail, how the main list is ordered, and how a drag persists given the server
// only knows per-project session order plus a global project order.

import { sortedSessionIds, type SortKey as BaseSortKey } from "@/lib/sortSessions"
import type { SessionView } from "@/lib/types"

// The flat list's display sort. "active" is the default (core order with
// working / needs-attention agents floated to the top); the remaining keys reuse
// the exact TUI-parity comparators; "manual" is the raw persisted order, and the
// ONLY mode in which drag-reorder is offered (a computed sort would just snap a
// dragged row back, and an active-first order is dynamic so it cannot persist).
export type FlatSortKey = "active" | BaseSortKey | "manual"

export const FLAT_SORT_LABELS: Record<FlatSortKey, string> = {
  active: "Active first",
  updated: "Recently updated",
  created: "Recently created",
  name: "Name (A to Z)",
  // The web picker does not OFFER name_desc (only the TUI cycles into it), but
  // the web must DISPLAY it when the TUI set it, so it needs a label.
  name_desc: "Name (Z to A)",
  manual: "Manual order",
}

// A session is "quiet" when it is not active (detached or exited): dormant work
// that collapses into the Quiet tail instead of hogging the main list.
export function isQuietSession(session: SessionView): boolean {
  return session.status !== "active"
}

// Split the sessions into the always-visible MAIN list (active agents) and the
// collapsible QUIET tail (detached / exited). Input order is preserved within
// each bucket so a later sort/partition sees a stable starting order.
export function partitionQuiet(sessions: SessionView[]): {
  main: SessionView[]
  quiet: SessionView[]
} {
  const main: SessionView[] = []
  const quiet: SessionView[] = []
  for (const session of sessions) {
    if (isQuietSession(session)) quiet.push(session)
    else main.push(session)
  }
  return { main, quiet }
}

// Stable active-first ordering: agents that are working or need attention rise
// above the rest, each group keeping the caller's incoming (core) order. This is
// a lightweight float, not a full re-sort, the underlying order is untouched, so
// it composes cleanly over the persisted drag order.
export function activeFirstSessions(sessions: SessionView[]): SessionView[] {
  const hot: SessionView[] = []
  const rest: SessionView[] = []
  for (const session of sessions) {
    if (session.working || session.needs_attention) hot.push(session)
    else rest.push(session)
  }
  return [...hot, ...rest]
}

// Order the MAIN sessions for display by the chosen sort. "active" floats the hot
// agents up (default); "manual" is the caller's order verbatim (the persisted
// drag order); the remaining keys reuse `sortedSessionIds` so the flat list sorts
// identically to the TUI palette commands.
export function sortMainSessions(
  sessions: SessionView[],
  key: FlatSortKey,
): SessionView[] {
  if (key === "active") return activeFirstSessions(sessions)
  if (key === "manual") return sessions.slice()
  const order = sortedSessionIds(sessions, key as BaseSortKey)
  const byId = new Map(sessions.map((session) => [session.id, session]))
  return order
    .map((id) => byId.get(id))
    .filter((session): session is SessionView => session !== undefined)
}

// NOTE: agent order is now a single GLOBAL flat order (agents are independent of
// project grouping). A drag is a plain `moveItem` over the complete session id
// list, sent via `reorderAgents` — see FlatAgentList's handleDragEnd. The old
// project-aware `flatDragPlan` (same-project reorder vs cross-project block move)
// was removed with that change.

// The colored STATE WORD shown on a row's second line, the honest, field-backed
// stand-in for an "activity" string (dux has no such field). It reads straight
// off the same flags that drive the bob and the attention pulse, so the word and
// the motion cue can never disagree. Colors are Tailwind palette utilities, the
// established pattern in agentRow.ts (never raw hex/oklch).
export interface StateWord {
  label: string
  className: string
}

export function stateWord(session: SessionView): StateWord {
  if (session.needs_attention) return { label: "Needs you", className: "text-cyan-100" }
  if (session.status === "active" && session.working) {
    // Match the app's active status color (agentRow.ts STATUS_DOT_COLOR),
    // not a new palette hue.
    return { label: "Working", className: "text-green-500" }
  }
  if (session.status === "active") {
    return { label: "Idle", className: "text-muted-foreground" }
  }
  if (session.status === "detached") {
    return { label: "Detached", className: "text-amber-500" }
  }
  return { label: "Exited", className: "text-muted-foreground" }
}
