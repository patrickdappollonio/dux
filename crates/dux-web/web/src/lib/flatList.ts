// Pure helpers backing the flat agents list, shared by the desktop sidebar, the
// collapsed rail and the mobile hub so the surfaces never drift. Kept free of
// React and dnd-kit so every rule here is unit-testable.
//
// The list is one globally ordered list: active agents and the project and
// standalone terminals form the main bucket, sorted together, and detached or
// exited agents with none of their own terminals running form the collapsible
// Inactive tail. An agent's own terminals are not rows of it. The ordering is a
// twin of the core-owned `dux_core::flat_list::order_rows`, pinned by the shared
// fixture `crates/dux-core/tests/fixtures/flat_list_cross_language.json`, which
// flatList.test.ts reads.

import { agentRowKey, terminalRowKey } from "@/hooks/use-reveal-selected-row"
import { sessionLabel } from "@/lib/agentWorkspace"
import {
  compareCodePoints,
  epoch,
  type SortKey as BaseSortKey,
} from "@/lib/sortSessions"
import { reorderById } from "@/lib/reorder"
import { matchWireOwner } from "@/lib/terminalOwner"
import type { SessionView, TerminalView } from "@/lib/types"

// The flat list's display sort. "active" is the default (stored order with
// working / needs-attention agents and running terminals floated to the top);
// the remaining keys reuse the TUI-parity comparators; "manual" is the stored
// order with each terminal placed after its anchor agent.
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

// One row of the list's main bucket: an agent, or a project or standalone
// terminal.
export type FlatRow =
  | { kind: "agent"; session: SessionView }
  | { kind: "terminal"; terminal: TerminalView }

// The ordered list: the main bucket of agents and terminals together, then the
// Inactive tail, which holds agents only.
export interface FlatRows {
  main: FlatRow[]
  inactive: SessionView[]
}

// The row's key, the same one the sidebar's reveal and the search pick use.
export function flatRowKey(row: FlatRow): string {
  return row.kind === "agent"
    ? agentRowKey(row.session.id)
    : terminalRowKey(row.terminal.id)
}

// Whether the agent ALONE is inactive: detached or exited. This is the question
// the pull-request poller asks of an agent, and the twin of
// `dux_core::flat_list::is_inactive`; the list's tail asks `inInactiveTail`,
// which also keeps an agent whose own terminals are still running out of it.
// Nothing checks that the two twins agree, so change one and change the other
// in the same commit.
export function isQuietSession(session: SessionView): boolean {
  return session.status !== "active"
}

// Whether an agent belongs in the list's Inactive tail: detached or exited, and
// with none of its own terminals running. An agent stopped with its terminals
// left going is still in use, so it stays in the main list until its last
// terminal ends. Twin of `dux_core::flat_list::in_inactive_tail`.
export function inInactiveTail(
  session: SessionView,
  hasRunningTerminal: boolean,
): boolean {
  return isQuietSession(session) && !hasRunningTerminal
}

// Whether a terminal is a row of the list: a project or standalone terminal. An
// agent's own terminal lives with its agent instead.
export function isListTerminal(terminal: TerminalView): boolean {
  return matchWireOwner(terminal.owner, {
    session: () => false,
    project: () => true,
    standalone: () => true,
  })
}

// The agents with at least one terminal of their own. A terminal exists only
// while its process runs, so having one is having a running one.
export function sessionsWithOwnTerminal(
  terminals: readonly TerminalView[],
): Set<string> {
  const ids = new Set<string>()
  for (const terminal of terminals) {
    matchWireOwner(terminal.owner, {
      session: (owner) => ids.add(owner.session_id),
      project: () => undefined,
      standalone: () => undefined,
    })
  }
  return ids
}

// Whether a quiet tail renders forced-open for the current search. Twin of
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

// The main bucket in incoming order: the given agents in stored order, each list
// terminal right after the agent its `list_after` names, terminals naming the
// same agent in their input order, one naming no agent first and one naming an
// agent that is gone last. Anchored to the STORED order, so a terminal keeps
// its place whether its anchor is on screen or in the tail.
function interleaveTerminals(
  sessions: readonly SessionView[],
  shownAgents: ReadonlySet<string>,
  listTerminals: readonly TerminalView[],
): FlatRow[] {
  const byAnchor = new Map<string | null, TerminalView[]>()
  for (const terminal of listTerminals) {
    const anchor = terminal.list_after ?? null
    const group = byAnchor.get(anchor)
    if (group) group.push(terminal)
    else byAnchor.set(anchor, [terminal])
  }
  const rows: FlatRow[] = []
  const place = (anchor: string | null) => {
    for (const terminal of byAnchor.get(anchor) ?? []) {
      rows.push({ kind: "terminal", terminal })
    }
    byAnchor.delete(anchor)
  }
  place(null)
  for (const session of sessions) {
    if (shownAgents.has(session.id)) rows.push({ kind: "agent", session })
    place(session.id)
  }
  for (const orphans of byAnchor.values()) {
    for (const terminal of orphans) rows.push({ kind: "terminal", terminal })
  }
  return rows
}

function rowName(row: FlatRow): string {
  // A terminal sorts by the name it was opened with, never its live title, so
  // a program renaming its window does not move the row.
  return row.kind === "agent"
    ? sessionLabel(row.session).toLowerCase()
    : row.terminal.label.toLowerCase()
}

function rowUpdated(row: FlatRow): number {
  return epoch(row.kind === "agent" ? row.session.updated_at : row.terminal.updated_at)
}

function rowCreated(row: FlatRow): number {
  return epoch(row.kind === "agent" ? row.session.created_at : row.terminal.created_at)
}

// Working or needing you for an agent; running (busy or being typed into) for a
// terminal.
function rowHot(row: FlatRow): boolean {
  return row.kind === "agent"
    ? row.session.working || row.session.needs_attention
    : row.terminal.working || row.terminal.typing
}

// The list's inputs with every drag still in flight laid over the spine: the
// agents' order, the terminals' order and each list terminal's place. The
// sidebar list, the collapsed rail and the store's next-agent pick start from
// it, so none of them shows a different order while a drop is unconfirmed.
export function overlaidListInputs(input: {
  sessions: readonly SessionView[]
  terminals: readonly TerminalView[]
  pendingAgentOrder: string[] | null
  pendingTerminalOrder: string[] | null
  // Absent means nothing is in flight.
  pendingTerminalPlacement?: Readonly<Record<string, string | null>>
}): { sessions: SessionView[]; terminals: TerminalView[] } {
  const sessions = input.pendingAgentOrder
    ? reorderById(input.sessions.slice(), input.pendingAgentOrder)
    : input.sessions.slice()
  const ordered = input.pendingTerminalOrder
    ? reorderById(input.terminals.slice(), input.pendingTerminalOrder)
    : input.terminals.slice()
  const placement = input.pendingTerminalPlacement ?? {}
  const terminals = ordered.map((terminal) =>
    terminal.id in placement
      ? { ...terminal, list_after: placement[terminal.id] }
      : terminal,
  )
  return { sessions, terminals }
}

// Order the agents and the project and standalone terminals for the list.
// `sessions` is the stored agent order (any pending drag already laid over it)
// and `terminals` every terminal: an agent's own are read only to keep it out of
// the tail. Nothing passed in is mutated.
//
// - "active": hot agents and running terminals float up, stably; the tail sorts
//   most recently updated first.
// - "updated" / "created" / "name" / "name_desc": the main bucket sorts by that
//   comparator over agents and terminals together; the tail stays verbatim.
// - "manual": both buckets verbatim.
//
// Terminals are never in the tail.
export function orderFlatRows(
  sessions: readonly SessionView[],
  terminals: readonly TerminalView[],
  key: FlatSortKey,
): FlatRows {
  const withTerminal = sessionsWithOwnTerminal(terminals)
  const shown = new Set<string>()
  let inactive: SessionView[] = []
  for (const session of sessions) {
    if (inInactiveTail(session, withTerminal.has(session.id))) inactive.push(session)
    else shown.add(session.id)
  }
  let main = interleaveTerminals(sessions, shown, terminals.filter(isListTerminal))

  switch (key) {
    case "active": {
      main = [...main.filter(rowHot), ...main.filter((row) => !rowHot(row))]
      inactive = inactive
        .slice()
        .sort((a, b) => epoch(b.updated_at) - epoch(a.updated_at))
      break
    }
    case "manual":
      break
    case "updated":
      main.sort((a, b) => rowUpdated(b) - rowUpdated(a))
      break
    case "created":
      main.sort((a, b) => rowCreated(b) - rowCreated(a))
      break
    case "name":
      main.sort((a, b) => compareCodePoints(rowName(a), rowName(b)))
      break
    case "name_desc":
      main.sort((a, b) => -compareCodePoints(rowName(a), rowName(b)))
      break
  }
  return { main, inactive }
}

// The agent to land on when the focused one vanishes. "Next" is read off the
// ordering the user is looking at, the main bucket in the current sort mode:
// `previous` is the list as it was while the gone agent still existed, which
// gives next a position to count from, and `current` is the list that just
// arrived. The scan wraps, so deleting the last row lands on the first. Only
// active agents are candidates, since a dormant one has no process to land in;
// null when none is left, which the caller renders as home. `terminals` decides
// which stopped agents still sit in the main bucket.
export function nextActiveSessionId(
  previous: SessionView[],
  current: SessionView[],
  goneSessionId: string,
  key: FlatSortKey,
  terminals: readonly TerminalView[] = [],
): string | null {
  const mainAgents = (sessions: SessionView[]) =>
    orderFlatRows(sessions, terminals, key).main.flatMap((row) =>
      row.kind === "agent" ? [row.session] : [],
    )
  const candidates = mainAgents(current)
    .filter((session) => !isQuietSession(session))
    .map((session) => session.id)
  if (candidates.length === 0) return null
  const candidateSet = new Set(candidates)
  const before = mainAgents(previous).map((session) => session.id)
  const at = before.indexOf(goneSessionId)
  // The gone agent was not in the main bucket (it was in the tail, or this
  // client never saw it): there is no position to count from, so take the first
  // row.
  if (at === -1) return candidates[0]
  for (let step = 1; step <= before.length; step++) {
    const id = before[(at + step) % before.length]
    if (candidateSet.has(id)) return id
  }
  // Every agent that shared the old main bucket is gone too; fall back to
  // whatever the new list starts with.
  return candidates[0]
}

// The complete row order a drop is computed against: every agent and every list
// terminal, in the order the user is looking at. Every row is included, never the
// search-filtered subset, because the stored order is total. Under a computed
// sort it is the displayed order, the main bucket then the tail; under "manual"
// it is the stored order with the terminals interleaved, tail agents left where
// the stored order has them. Twin of the terminal UI's `list_drag_baseline`.
export function listDragBaseline(
  sessions: readonly SessionView[],
  terminals: readonly TerminalView[],
  key: FlatSortKey,
): FlatRow[] {
  if (key === "manual") {
    return interleaveTerminals(
      sessions,
      new Set(sessions.map((session) => session.id)),
      terminals.filter(isListTerminal),
    )
  }
  const { main, inactive } = orderFlatRows(sessions, terminals, key)
  return [
    ...main,
    ...inactive.map((session): FlatRow => ({ kind: "agent", session })),
  ]
}

// What a drop asks the server for. Each part is null (or empty) when it changes
// nothing, so a drop back where it started sends nothing at all.
export interface ListDropPlan {
  // The complete agent order, when it moved.
  agentOrder: string[] | null
  // Each list terminal whose anchor moved, with its new anchor.
  placements: Record<string, string | null>
  // The complete terminal order (the list's terminals in their new order, then
  // every agent's own terminals in their current order), when the list's
  // terminals changed their order among themselves.
  terminalOrder: string[] | null
}

// Move the row `activeKey` to the slot of `overKey` in `baseline` and work out
// what that asks of the server: the agents' new order, each list terminal placed
// after the agent above it (or first), and the terminals' own order. The twin of
// the terminal UI's `apply_list_order`. Null when the move changes nothing.
// `sessions` and `terminals` are the stored orders, any drag in flight already
// laid over them.
export function planListDrop(
  baseline: readonly FlatRow[],
  activeKey: string,
  overKey: string,
  sessions: readonly SessionView[],
  terminals: readonly TerminalView[],
): ListDropPlan | null {
  const from = baseline.findIndex((row) => flatRowKey(row) === activeKey)
  const to = baseline.findIndex((row) => flatRowKey(row) === overKey)
  if (from === -1 || to === -1 || from === to) return null
  const next = baseline.slice()
  const [moved] = next.splice(from, 1)
  next.splice(to, 0, moved)

  const agentOrder = next.flatMap((row) => (row.kind === "agent" ? [row.session.id] : []))
  const stored = sessions.map((session) => session.id)
  const agentsMoved =
    agentOrder.length !== stored.length || agentOrder.some((id, i) => id !== stored[i])

  const placements: Record<string, string | null> = {}
  const listOrder: string[] = []
  let after: string | null = null
  for (const row of next) {
    if (row.kind === "agent") {
      after = row.session.id
      continue
    }
    listOrder.push(row.terminal.id)
    if ((row.terminal.list_after ?? null) !== after) placements[row.terminal.id] = after
  }
  // Only the list terminals' relative order is the list's business; an agent's
  // own terminals keep theirs, which is their dock's order.
  const listed = new Set(listOrder)
  const current = terminals.map((terminal) => terminal.id)
  const currentListed = current.filter((id) => listed.has(id))
  const terminalsMoved = listOrder.some((id, i) => id !== currentListed[i])
  const terminalOrder = [...listOrder, ...current.filter((id) => !listed.has(id))]

  if (!agentsMoved && !terminalsMoved && Object.keys(placements).length === 0) {
    return null
  }
  return {
    agentOrder: agentsMoved ? agentOrder : null,
    placements,
    terminalOrder: terminalsMoved ? terminalOrder : null,
  }
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
    // The working token, which shares the active status dot's green
    // (agentRow.ts STATUS_DOT_COLOR) today.
    return { label: "Working", className: "text-dux-working" }
  }
  if (session.status === "active") {
    return { label: "Idle", className: "text-muted-foreground" }
  }
  if (session.status === "detached") {
    return { label: "Detached", className: "text-dux-detached" }
  }
  return { label: "Exited", className: "text-muted-foreground" }
}
