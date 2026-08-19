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
//
// The ORDERING (main-bucket sort + the Active-mode recency-sorted quiet tail) is
// a TWIN of the core-owned `dux_core::flat_list::order_sessions`, pinned by
// shared vectors (`flatList.test.ts` mirrors `flat_list.rs`).

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
  // the web must DISPLAY it when the TUI set it, so it needs a label. Where it
  // is displayed changed with the static sort trigger: the trigger now always
  // reads "Sort", so the sort MENU appends a checked "Name (Z to A)" row while
  // that mode is active (the touch-visible truth), and the trigger's tooltip
  // names the mode as a desktop nicety on top of it.
  name_desc: "Name (Z to A)",
  manual: "Manual order",
}

// A session is "quiet" when it is not active (detached or exited): dormant work
// that collapses into the Quiet tail instead of hogging the main list.
export function isQuietSession(session: SessionView): boolean {
  return session.status !== "active"
}

// Whether the Quiet tail should render forced-open for the current search. TWIN
// of the core-owned `dux_core::quiet_tail::quiet_tail_forced_open` (the DECISION),
// pinned by shared vectors. `normalizedQuery` is the trimmed/lowercased query
// (empty means no active search); `dismissedQuery` is the NORMALIZED query under
// which the user last collapsed the search-expanded tail; `hasQuietHit` is
// whether the query matches a dormant row. Keying on the normalized query means a
// whitespace/case variant of a dismissed query does NOT resurrect the tail.
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

// Order the QUIET (inactive) tail for display. In "active" mode the tail sorts
// MOST-RECENTLY-ACTIVE-FIRST (Reverse(updated_at)), matching the TUI's
// build_left_items / the core-owned `flat_list::order_sessions`; every other mode
// leaves the tail VERBATIM (only "active" reorders the tail, so the surfaces
// agree). TWIN of the core ordering, pinned by shared vectors.
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

// The agent to land on when the focused one vanishes (deleted here or by another
// client). "Next" is read off the SAME ordering the user is looking at: the main
// (active) bucket in the current sort mode, exactly what `FlatAgentList` renders
// above the quiet tail. `previous` is the session list as it was while the gone
// agent still existed, which is what gives "next" a position to count from;
// `current` is the list that just arrived. The scan starts after the gone agent
// and wraps, so deleting the last row lands on the first one rather than on
// nothing. Only ACTIVE agents are candidates: a detached or exited agent has no
// live process to land in, so it stays in the quiet tail where it belongs.
// Returns null when no active agent is left, which the caller renders as home.
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

// NOTE: agent order is now a single GLOBAL flat order (agents are independent of
// project grouping). A drag is a plain `moveItem` over the complete session id
// list, sent via `reorderAgents` — see FlatAgentList's handleDragEnd. The old
// project-aware `flatDragPlan` (same-project reorder vs cross-project block move)
// was removed with that change.

// The drag baseline for a drop: the COMPLETE session id list in the order the
// user is actually looking at. Drag-reorder works from every sort mode; on a
// drop made in a computed mode (active/name/updated/created) the new manual
// baseline must be "what the screen showed, totalized": the main list in the
// active sort's display order, then the quiet tail (which renders below the
// main list) in its base relative order. Every session is included, never just
// the visible/filtered subset, because the persisted order is total. MANUAL is
// deliberately the base order VERBATIM (quiet sessions stay interleaved where
// the base has them): that is exactly how manual drags always computed their
// move, and drag-from-any-mode must not change manual's behavior.
export function displayedSessionOrder(
  sessions: SessionView[],
  key: FlatSortKey,
): string[] {
  if (key === "manual") return sessions.map((session) => session.id)
  const { main, quiet } = partitionQuiet(sessions)
  // The tail rides below the main list, ordered by `sortQuietTail` (recency in
  // "active" mode, verbatim otherwise) so the persisted drag baseline matches
  // exactly what the screen shows.
  return [...sortMainSessions(main, key), ...sortQuietTail(quiet, key)].map(
    (session) => session.id,
  )
}

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
  // TWIN of the core-owned priority ladder `dux_core::row_state::agent_row_state`
  // (the DECISION); this surface only words and colors it. Pinned by shared
  // vectors (`flatList.test.ts` mirrors `row_state.rs`'s tests). The order:
  // needs-attention wins; then for an active agent typing outranks working,
  // working outranks idle; then the non-active detached/exited words.
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
