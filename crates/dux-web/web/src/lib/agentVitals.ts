// Pure model builder for the "full vitals" agent tooltip, shared by the collapsed icon rail
// and the expanded sidebar agent rows (components/AgentVitalsTooltip.tsx).

import { statusDotColorClass } from "@/lib/agentRow"
import { formatRegularCount } from "@/lib/formatRegularCount"
import type { ChangesSlice } from "@/lib/store"
import type { SessionView } from "@/lib/types"
import {
  folderWorkspace,
  managedWorkspace,
  sessionLabel,
} from "@/lib/agentWorkspace"

export interface AgentVitalsRow {
  key: string
  label: string
  value: string
  mono?: boolean
}

export interface AgentVitalsModel {
  name: string
  provider: string
  statusLabel: string
  statusColorClass: string
  projectName: string
  rows: AgentVitalsRow[]
}

// The status line's label, mirroring StatusBadge's precedence: needs_attention, then
// working, then active; anything else is the raw status word, capitalized.
function vitalsStatusLabel(session: SessionView): string {
  if (session.needs_attention) return "Needs attention"
  if (session.status === "active" && session.working) return "Working"
  if (session.status === "active") return "Active"
  return session.status.charAt(0).toUpperCase() + session.status.slice(1)
}

// Live/total tab count ("2 of 3 live"), or null for a single-tab agent, whose liveness the
// status line already shows.
function tabsSummary(session: SessionView): string | null {
  const total = session.tabs.length
  if (total <= 1) return null
  const live = session.tabs.filter((t) => t.has_live_process).length
  return `${live} of ${total} live`
}

// The tabs' providers in first-appearance order, each counted when it holds more than one
// tab (`claude (2), codex`). Tab provider strings arrive already resolved.
export function providersSummary(session: SessionView): string {
  if (session.tabs.length === 0) return session.provider
  const counts = new Map<string, number>()
  for (const tab of session.tabs) {
    counts.set(tab.provider, (counts.get(tab.provider) ?? 0) + 1)
  }
  return [...counts.entries()]
    .map(([name, n]) => (n > 1 ? `${name} (${n})` : name))
    .join(", ")
}

// Plain current branch, or an "initial → current" drift form when the worktree has moved off
// the branch the agent was created on. `null` for a standalone agent, which has no branch.
function branchValue(session: SessionView): string | null {
  const managed = managedWorkspace(session.workspace)
  if (!managed) return null
  if (
    managed.initial_branch &&
    managed.initial_branch !== managed.branch_name
  ) {
    return `${managed.initial_branch} → ${managed.branch_name}`
  }
  return managed.branch_name
}

// The changed-files store slice only ever holds the selected session's data, so any other
// session omits the count rather than showing another session's. Shared by both sidebar
// surfaces so they cannot drift.
export function changesCountFor(
  changes: ChangesSlice | null | undefined,
  sessionId: string,
): number | null {
  if (changes?.sessionId !== sessionId || changes.phase !== "loaded") {
    return null
  }
  return changes.staged.length + changes.unstaged.length
}

// Builds the vitals row model for one session. `changesCount` is this session's staged plus
// unstaged file count, or null when unavailable, and the row is then omitted.
export function buildAgentVitals(
  session: SessionView,
  projectName: string,
  changesCount: number | null,
): AgentVitalsModel {
  const rows: AgentVitalsRow[] = []

  const branch = branchValue(session)
  if (branch) rows.push({ key: "branch", label: "Branch", value: branch, mono: true })

  const managed = managedWorkspace(session.workspace)
  // The branch this agent was forked from. Skipped when it matches the current
  // branch, where it would say nothing the branch row doesn't.
  if (
    managed &&
    managed.source_branch &&
    managed.source_branch !== managed.branch_name
  ) {
    rows.push({
      key: "source",
      label: "Source",
      value: managed.source_branch,
      mono: true,
    })
  }

  // A standalone agent's folder. The managed shape has no directory row, because a worktree
  // is named after its branch, but a user's own folder is named nothing in particular.
  const folder = folderWorkspace(session.workspace)
  if (folder) {
    rows.push({
      key: "folder",
      label: "Folder",
      value: folder.folder_label,
      mono: true,
    })
  }

  if (changesCount !== null && changesCount > 0) {
    rows.push({
      key: "changes",
      label: "Changes",
      value: formatRegularCount(changesCount, "file"),
    })
  }

  const tabs = tabsSummary(session)
  if (tabs) rows.push({ key: "tabs", label: "Tabs", value: tabs })

  if (session.pr) {
    rows.push({
      key: "pr",
      label: "PR",
      value: `#${session.pr.number} ${session.pr.state}`,
    })
  }

  // No worktree row: worktree directories are named after the branch, so the
  // branch row above already identifies it without repeating a long path.

  return {
    name: sessionLabel(session),
    provider: providersSummary(session),
    statusLabel: vitalsStatusLabel(session),
    statusColorClass: statusDotColorClass(
      session.status,
      session.needs_attention,
    ),
    projectName,
    rows,
  }
}
