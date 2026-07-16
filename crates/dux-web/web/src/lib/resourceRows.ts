// The pure join behind the Task Manager: reconcile the sampled stats against the
// live spine and produce the exact row list to render.
//
// Two rules drive everything here:
//
//  1. **The spine is authoritative for existence.** A stat with no spine row is
//     an orphan (the runtime was killed between the poll and the refetch) and is
//     dropped. A spine row with no stat still renders, with dashes, and stays
//     stoppable: never drop a killable row for lack of numbers.
//  2. **Join by id, never by label.** Core stamps each sampled row with the tab
//     or terminal id it came from precisely so this join is exact. Matching on a
//     label like "Agent (claude): fix-auth" breaks on a title containing "): "
//     and silently conflates two agents that share a title.
//
// Order is deterministic and never depends on a stat value (R7): dux, then each
// agent's session-slot tab with its extra tabs nested by `order`, then that
// agent's terminals, then TOTAL. Sorting by CPU would reorder rows under the
// user's cursor on every poll.

import type { ResourceStatsView } from "./resourcesApi"
import { terminalTitle } from "./terminals"
import type { SessionView } from "./types"

export type TaskRowKind = "dux" | "agent" | "terminal" | "total"

export interface TaskRow {
  /** Stable React key and test handle: `dux`, `total`, `tab:<id>`, `term:<id>`. */
  key: string
  kind: TaskRowKind
  /** The primary display name. Never truncated to nothing by the renderer. */
  name: string
  /** Secondary text (provider, foreground command), or null. */
  detail: string | null
  /** An extra tab, rendered indented under its agent's session-slot tab. */
  nested: boolean
  /** Whether this row offers a Stop control (dux and TOTAL do not). */
  stoppable: boolean
  /** The Stop control's accessible name. Distinct from `name`: a nested extra
   * tab's `name` is only its provider ("claude"), which collides across
   * sibling tabs of the same provider (a supported configuration, see
   * CLAUDE.md's agent-tabs tenets) and carries no agent identity on its own.
   * This field always includes the owning agent and the tab's position so it
   * stays unique and meaningful even when `name` alone would not. */
  stopLabel: string
  /** The owning session, for the stop action. Null for dux/total. */
  sessionId: string | null
  /** The tab id or terminal id to act on. Null for dux/total. */
  targetId: string | null
  /** The sampled numbers, or null when this row had no stats this poll (a
   * dormant tab, or a process born since the last sample). */
  stats: ResourceStatsView | null
}

// Build the Task Manager's rows from the spine and the latest sample.
//
// `sessions` is the spine's session list; `stats` the rows from
// `GET /api/v1/resources` (possibly empty before the first poll lands, in which
// case every row renders with dashes).
export function taskManagerRows(
  sessions: readonly SessionView[],
  stats: readonly ResourceStatsView[],
): TaskRow[] {
  // Index the sampled rows by the id core stamped on them.
  const byId = new Map<string, ResourceStatsView>()
  for (const s of stats) {
    if (s.id !== null) byId.set(s.id, s)
  }

  const rows: TaskRow[] = []

  const dux = stats.find((s) => s.kind === "dux") ?? null
  rows.push({
    key: "dux",
    kind: "dux",
    name: "dux",
    detail: null,
    nested: false,
    // dux is the app you are looking at: there is nothing to tell the user, so
    // the renderer shows a muted dash rather than a disabled button.
    stoppable: false,
    stopLabel: "dux",
    sessionId: null,
    targetId: null,
    stats: dux,
  })

  for (const session of sessions) {
    const sessionLabel = session.title ?? session.branch_name

    // Only agent TABS gate on liveness: a detached/exited agent's tabs have no
    // live PTY, so they are not a running task (matching the modal this
    // replaces). This gate must NOT reach the terminals loop below.
    if (session.status === "active") {
      // The session-slot tab (id === session id) leads the group; extra tabs
      // nest under it in creation order. `sort_order` is append-only, so
      // `order` is a stable sort key.
      const tabs = [...session.tabs].sort((a, b) => {
        if (a.id === session.id) return -1
        if (b.id === session.id) return 1
        return a.order - b.order
      })

      // 1-based position among this session's EXTRA tabs, in the same stable
      // order, so two same-provider tabs never share a Stop label (finding 4).
      let nestedIndex = 0

      for (const tab of tabs) {
        const isSlot = tab.id === session.id
        if (!isSlot) nestedIndex += 1
        rows.push({
          key: `tab:${tab.id}`,
          kind: "agent",
          // The slot tab carries the agent's identity; an extra tab is
          // identified by the provider running in it.
          name: isSlot ? sessionLabel : tab.provider,
          detail: isSlot ? tab.provider : null,
          nested: !isSlot,
          // A dormant tab has no process but is still closeable, so it keeps
          // its Stop control.
          stoppable: true,
          stopLabel: isSlot
            ? `Stop ${sessionLabel}`
            : `Stop ${tab.provider} tab ${nestedIndex} in ${sessionLabel}`,
          sessionId: session.id,
          targetId: tab.id,
          stats: byId.get(tab.id) ?? null,
        })
      }
    }

    // Companion terminals are independent of the owning session's status:
    // detaching an agent DELIBERATELY leaves its terminals running (a live
    // PTY the user may still want to reach or stop), so this loop must never
    // sit inside the `status === "active"` gate above. Every terminal in the
    // spine is a live PTY (terminals are never persisted dormant), so
    // existence always means running, regardless of the agent's own status.
    for (const terminal of session.terminals) {
      const title = terminalTitle(terminal, session.terminals)
      rows.push({
        key: `term:${terminal.id}`,
        kind: "terminal",
        name: title,
        detail: sessionLabel,
        nested: false,
        stoppable: true,
        stopLabel: `Stop ${title}`,
        sessionId: session.id,
        targetId: terminal.id,
        stats: byId.get(terminal.id) ?? null,
      })
    }
  }

  const total = stats.find((s) => s.kind === "total") ?? null
  rows.push({
    key: "total",
    kind: "total",
    name: "TOTAL",
    detail: null,
    nested: false,
    stoppable: false,
    stopLabel: "TOTAL",
    sessionId: null,
    targetId: null,
    stats: total,
  })

  return rows
}

// Whether the Task Manager has anything to stop. The dux and TOTAL rows always
// render, so "nothing is running" means no agents and no terminals.
export function nothingRunning(rows: readonly TaskRow[]): boolean {
  return !rows.some((r) => r.stoppable)
}
