// Pure helpers backing the flat agent list, shared by the desktop sidebar and
// the mobile hub so the two surfaces never drift. Kept free of React and
// dnd-kit so every rule here is unit-testable.
//
// The ordering (main-bucket sort plus the Active-mode recency-sorted quiet
// tail) is a twin of the core-owned `dux_core::flat_list::order_sessions`,
// pinned by shared vectors.

import { sortedSessionIds, type SortKey as BaseSortKey } from "@/lib/sortSessions"
import type { SessionView } from "@/lib/types"

// The flat list's display sort. "active" is the default (core order with
// working / needs-attention agents floated to the top); the remaining keys
// reuse the TUI-parity comparators; "manual" is the raw persisted order.
export type FlatSortKey = "active" | BaseSortKey | "manual"

export const FLAT_SORT_LABELS: Record<FlatSortKey, string> = {
  active: "Active first",
  updated: "Recently updated",
  created: "Recently created",
  name: "Name (A to Z)",
  // The web picker never offers name_desc (only the TUI cycles into it) but has
  // to display it when the TUI set it: the menu appends a checked row for it
  // while it is active, since the trigger always reads "Sort".
  name_desc: "Name (Z to A)",
  manual: "Manual order",
}

// A session is "quiet" when it is not active (detached or exited): dormant work
// that collapses into the Quiet tail instead of hogging the main list.
export function isQuietSession(session: SessionView): boolean {
  return session.status !== "active"
}

// Whether the Quiet tail renders forced-open for the current search. Twin of
// the core-owned `dux_core::quiet_tail::quiet_tail_forced_open`, pinned by
// shared vectors. Both queries are trimmed and lowercased (empty means no
// search, `dismissedQuery` is the one the user last collapsed the tail under),
// so a whitespace or case variant does not resurrect a dismissed tail.
export function quietTailForcedOpen(
  normalizedQuery: string,
  dismissedQuery: string | null,
  hasQuietHit: boolean,
): boolean {
  if (normalizedQuery === "") return false
  if (dismissedQuery === normalizedQuery) return false
  return hasQuietHit
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

// Order the quiet (inactive) tail for display: in "active" mode most recently
// updated first, every other mode verbatim, matching the core-owned
// `flat_list::order_sessions` so the surfaces agree.
export function sortQuietTail(
  sessions: SessionView[],
  key: FlatSortKey,
): SessionView[] {
  if (key !== "active") return sessions.slice()
  const order = sortedSessionIds(sessions, "updated")
  const byId = new Map(sessions.map((session) => [session.id, session]))
  return order
    .map((id) => byId.get(id))
    .filter((session): session is SessionView => session !== undefined)
}

// The agent to land on when the focused one vanishes. "Next" is read off the
// ordering the user is looking at, the main bucket in the current sort mode:
// `previous` is the list as it was while the gone agent still existed, which
// gives next a position to count from, and `current` is the list that just
// arrived. The scan wraps, so deleting the last row lands on the first. Only
// active agents are candidates, since a dormant one has no process to land in;
// null when none is left, which the caller renders as home.
export function nextActiveSessionId(
  previous: SessionView[],
  current: SessionView[],
  goneSessionId: string,
  key: FlatSortKey,
): string | null {
  const candidates = sortMainSessions(partitionQuiet(current).main, key).map(
    (session) => session.id,
  )
  if (candidates.length === 0) return null
  const candidateSet = new Set(candidates)
  const before = sortMainSessions(partitionQuiet(previous).main, key).map(
    (session) => session.id,
  )
  const at = before.indexOf(goneSessionId)
  // The gone agent was not in the active bucket (it was quiet, or this client
  // never saw it): there is no position to count from, so take the first row.
  if (at === -1) return candidates[0]
  for (let step = 1; step <= before.length; step++) {
    const id = before[(at + step) % before.length]
    if (candidateSet.has(id)) return id
  }
  // Every agent that shared the old active bucket is gone too; fall back to
  // whatever the new list starts with.
  return candidates[0]
}

// Agent order is one global flat list, persisted by `reorderAgents` as a plain
// move over every session id.

// The drag baseline for a drop: the complete session id list in the order the
// user is looking at. A drop made in a computed mode (active/name/updated/
// created) totalizes what the screen showed, the main list then the quiet tail
// below it. Every session is included, never the visible or filtered subset,
// because the persisted order is total. "manual" is deliberately the base order
// verbatim, quiet sessions interleaved where the base has them.
export function displayedSessionOrder(
  sessions: SessionView[],
  key: FlatSortKey,
): string[] {
  if (key === "manual") return sessions.map((session) => session.id)
  const { main, quiet } = partitionQuiet(sessions)
  return [...sortMainSessions(main, key), ...sortQuietTail(quiet, key)].map(
    (session) => session.id,
  )
}

// The colored state word on a row's second line, read off the same flags that
// drive the working pulse and the attention pulse so the word and the motion
// cue cannot disagree. Colors are Tailwind palette utilities, as in agentRow.ts, never raw
// hex or oklch.
export interface StateWord {
  label: string
  className: string
}

export function stateWord(session: SessionView): StateWord {
  // The priority ladder is the core-owned `dux_core::row_state::agent_row_state`
  // decision, pinned by shared vectors; this surface only words and colors it.
  if (session.needs_attention) return { label: "Needs you", className: "text-cyan-100" }
  if (session.status === "active" && session.typing) {
    // The soft-violet typing token, matching the TUI's `#c586e0` typing hue.
    return { label: "Typing", className: "text-dux-typing" }
  }
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
