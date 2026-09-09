// The pure join behind the Task Manager: the sampled stats reconciled against
// the live spine, as the exact row list to render. Three rules:
//
//  1. The spine is authoritative for existence. A stat with no spine row is an
//     orphan and is dropped; a spine row with no stat renders with dashes and
//     stays stoppable, since a killable row must never be dropped for want of
//     numbers.
//  2. Join by id, never by label. Core stamps each sampled row with its tab or
//     terminal id so the join is exact; a label like "Agent (claude): fix-auth"
//     breaks on a title containing "): " and conflates two agents sharing one.
//  3. Order never depends on a stat value: dux, then each agent's slot tab with
//     its extra tabs nested by `order`, then that agent's terminals, then TOTAL.
//     Sorting by CPU would reorder rows under the cursor on every poll.

import { formatBytes, formatCpu } from "./formatStats"
import type { ResourceStatsView } from "./resourcesApi"
import { matchWireOwner, ownerKey } from "./terminalOwner"
import { groupTerminalsByOwnerKey, terminalTitle } from "./terminals"
import type { ProjectView, SessionView, TerminalView } from "./types"
import { sessionLabel } from "@/lib/agentWorkspace"
import { isFirstTab } from "@/lib/agentTabs"

export type TaskRowKind = "dux" | "agent" | "terminal" | "total"

export interface TaskRow {
  /** Stable React key and test handle: `dux`, `total`, `tab:<id>`, `term:<id>`. */
  key: string
  kind: TaskRowKind
  /** The primary display name. Never truncated to nothing by the renderer. */
  name: string
  /** Secondary text (provider, foreground command), or null. */
  detail: string | null
  /** An extra tab, rendered indented under its agent's slot tab, and the row's
   * authoritative slot-ness answer: a consumer reads `!nested` rather than
   * deriving it again from the ids. False for dux, TOTAL and terminal rows. */
  nested: boolean
  /** Whether this row offers a Stop control (dux and TOTAL do not). */
  stoppable: boolean
  /** The Stop control's accessible name, which always carries the owning agent
   * and the tab's position. `name` cannot serve: a nested tab's is only its
   * provider, which collides across sibling tabs of the same provider. */
  stopLabel: string
  /** The owning session, for the stop action. Null for dux/total AND for a
   * project terminal (whose owner is a project, not a session). */
  sessionId: string | null
  /** The owning project, for a project-terminal row. Null everywhere else. */
  projectId: string | null
  /** The tab id or terminal id to act on. Null for dux/total. */
  targetId: string | null
  /** The sampled numbers, or null when this row had no stats this poll (a
   * dormant tab, or a process born since the last sample). */
  stats: ResourceStatsView | null
}

// Build the Task Manager's rows from the spine and the latest sample. `stats`
// may be empty before the first poll lands, in which case every row renders
// with dashes.
export function taskManagerRows(
  sessions: readonly SessionView[],
  stats: readonly ResourceStatsView[],
  projects: readonly ProjectView[],
  terminals: readonly TerminalView[],
): TaskRow[] {
  // Grouped by owner KEY, a total function of the owner, so no terminal can
  // fall out; the walks below emit each group where its owner sits.
  const terminalGroups = groupTerminalsByOwnerKey(terminals)
  // Index the sampled rows by the id core stamped on them.
  const byId = new Map<string, ResourceStatsView>()
  for (const s of stats) {
    if (s.id !== null) byId.set(s.id, s)
  }

  const rows: TaskRow[] = []

  const labels = new Map(sessions.map((s) => [s.id, sessionLabel(s)] as const))
  const projectNames = new Map(projects.map((p) => [p.id, p.name] as const))

  // One terminal's row, whose secondary text and stop action come from an
  // EXHAUSTIVE match, so a new owner kind is a compile error here. Deliberately
  // not `ownerSessionId`/`ownerProjectId`, which collapse an unknown owner into
  // a pair of nulls and keep compiling, rendering a row with no identity and no
  // way to act on it.
  const terminalRow = (
    terminal: TerminalView,
    group: readonly TerminalView[],
  ): TaskRow => {
    const owner = matchWireOwner<{
      detail: string | null
      sessionId: string | null
      projectId: string | null
    }>(terminal.owner, {
      session: (o) => ({
        detail: labels.get(o.session_id) ?? null,
        sessionId: o.session_id,
        projectId: null,
      }),
      project: (o) => ({
        detail: projectNames.get(o.project_id) ?? null,
        sessionId: null,
        projectId: o.project_id,
      }),
      // No owner, so the secondary text says where it is, as its sidebar row
      // does. Both ids are null; the stop action is keyed by `targetId`.
      standalone: (o) => ({
        detail: o.cwd_label,
        sessionId: null,
        projectId: null,
      }),
    })
    const title = terminalTitle(terminal, group)
    return {
      key: `term:${terminal.id}`,
      kind: "terminal",
      name: title,
      detail: owner.detail,
      nested: false,
      stoppable: true,
      stopLabel: `Stop ${title}`,
      sessionId: owner.sessionId,
      projectId: owner.projectId,
      targetId: terminal.id,
      stats: byId.get(terminal.id) ?? null,
    }
  }

  // Emit one owner's terminals, once. Groups are drained as they are emitted so
  // the sweep at the end can pick up anything the session and project walks
  // never reached.
  const emitted = new Set<string>()
  const emitTerminalGroup = (key: string) => {
    const group = terminalGroups.get(key)
    if (!group || emitted.has(key)) return
    emitted.add(key)
    for (const terminal of group) rows.push(terminalRow(terminal, group))
  }

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
    projectId: null,
    targetId: null,
    stats: dux,
  })

  for (const session of sessions) {
    rows.push(...agentTabRows(session, byId))

    // Never move this inside the `status === "active"` gate: detaching an agent
    // DELIBERATELY leaves its terminals running, and every terminal in the
    // spine is a live PTY, so existence means running whatever the agent's own
    // status is.
    emitTerminalGroup(ownerKey({ kind: "session", session_id: session.id }))
  }

  // Project terminals have no session, so no session's status gates them.
  for (const project of projects) {
    emitTerminalGroup(ownerKey({ kind: "project", project_id: project.id }))
  }

  // Standalone terminals have no owner section to sit under, so they are
  // emitted here to land in a predictable place. The sweep below would catch
  // them anyway, but as a safety net rather than the plan.
  emitTerminalGroup(ownerKey({ kind: "standalone", cwd_label: "" }))

  // Anything the walks above never reached, emitted rather than dropped: the
  // "Stop all" confirmation counts EVERY terminal in the flat collection, so a
  // terminal with no row leaves rows and count disagreeing about what is about
  // to be stopped, and takes away the only control for stopping it.
  for (const key of terminalGroups.keys()) emitTerminalGroup(key)

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
    projectId: null,
    targetId: null,
    stats: total,
  })

  return rows
}

// One agent's tab rows: the session-slot tab, then its extra tabs nested under
// it in creation order.
//
// Only agent TABS gate on liveness: a detached/exited agent's tabs have no live
// PTY, so they are not a running task (matching the modal this replaces). The
// gate must NOT reach that agent's terminals, which the caller emits.
export function agentTabRows(
  session: SessionView,
  byId: ReadonlyMap<string, ResourceStatsView>,
): TaskRow[] {
  if (session.status !== "active") return []
  const label = sessionLabel(session)
  // `sort_order` is append-only, so `order` is a stable sort key.
  const tabs = [...session.tabs].sort((a, b) => {
    if (isFirstTab(session, a.id)) return -1
    if (isFirstTab(session, b.id)) return 1
    return a.order - b.order
  })

  // 1-based position among this session's EXTRA tabs, in the same stable order,
  // so two same-provider tabs never share a Stop label.
  let nestedIndex = 0

  return tabs.map((tab) => {
    const isSlot = isFirstTab(session, tab.id)
    if (!isSlot) nestedIndex += 1
    return {
      key: `tab:${tab.id}`,
      kind: "agent" as const,
      // The slot tab carries the agent's identity; an extra tab is identified
      // by the provider running in it.
      name: isSlot ? label : tab.provider,
      detail: isSlot ? tab.provider : null,
      nested: !isSlot,
      // A dormant tab is still actionable, so it keeps its Stop control. WHICH
      // act that is rides on `nested`, which `handleStop` reads rather than
      // asking again: a first tab STOPS the agent, an extra tab is closed,
      // because a process monitor's Stop ends a process rather than deleting
      // the row it is showing numbers for.
      stoppable: true,
      stopLabel: isSlot
        ? `Stop ${label}`
        : `Stop ${tab.provider} tab ${nestedIndex} in ${label}`,
      sessionId: session.id,
      projectId: null,
      targetId: tab.id,
      stats: byId.get(tab.id) ?? null,
    }
  })
}

// Whether the Task Manager has anything to stop. The dux and TOTAL rows always
// render, so "nothing is running" means no agents and no terminals.
export function nothingRunning(rows: readonly TaskRow[]): boolean {
  return !rows.some((r) => r.stoppable)
}

// The footer's muted totals line, read straight off the TOTAL row rather than
// summed here: core computes that aggregate once, and re-deriving it would risk
// drifting from the collector's own rounding. `null` when nothing is running,
// so the summary disappears alongside the footer's "Stop all…".
export function taskManagerSummary(rows: readonly TaskRow[]): string | null {
  const runningCount = rows.filter((r) => r.stoppable).length
  if (runningCount === 0) return null

  const parts = [`${runningCount} running`]

  const total = rows.find((r) => r.kind === "total")?.stats ?? null
  if (total) {
    parts.push(`${total.process_count} process${total.process_count === 1 ? "" : "es"}`)
    parts.push(`${formatCpu(total.cpu_percent)} CPU`)
    parts.push(formatBytes(total.rss_bytes))
  }

  return parts.join(" · ")
}
