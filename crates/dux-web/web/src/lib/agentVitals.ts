// Pure, React-free model builder for the "full vitals" agent tooltip (shared by
// the collapsed icon rail and the expanded sidebar agent rows — see
// components/AgentVitalsTooltip.tsx). Kept here, framework-free, so the row
// selection/omission logic is trivially unit-testable without mounting a
// tooltip or a store.

import { statusDotColorClass } from "@/lib/agentRow"
import type { ChangesSlice } from "@/lib/store"
import type { SessionView } from "@/lib/types"

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

// The status line's label, mirroring StatusBadge's semantics: needs_attention
// wins over everything else (amber "Needs attention"); an active+working agent
// reads "Working" (green); a plain active agent reads "Active" (green); anything
// else (detached/exited) falls back to the raw status word, capitalized, in
// StatusBadge's muted/amber tone via `statusDotColorClass`.
function vitalsStatusLabel(session: SessionView): string {
  if (session.needs_attention) return "Needs attention"
  if (session.status === "active" && session.working) return "Working"
  if (session.status === "active") return "Active"
  return session.status.charAt(0).toUpperCase() + session.status.slice(1)
}

// Live/total tab count ("2 of 3 live"), or null when there is only one tab (a
// single-tab agent's liveness is already obvious from the status line, so the
// row is omitted entirely rather than showing a redundant "1 of 1").
function tabsSummary(session: SessionView): string | null {
  const total = session.tabs.length
  if (total <= 1) return null
  const live = session.tabs.filter((t) => t.has_live_process).length
  return `${live} of ${total} live`
}

// Branch row value: plain current branch, or an "initial → current" drift form
// when the worktree has moved off the branch the agent was created on.
function branchValue(session: SessionView): string {
  if (
    session.initial_branch &&
    session.initial_branch !== session.branch_name
  ) {
    return `${session.initial_branch} → ${session.branch_name}`
  }
  return session.branch_name
}

// The changed-files store slice only ever holds data for the currently
// SELECTED session (see ChangesSlice in lib/store.ts), so a row for any other
// session must omit the changes count rather than showing stale/wrong data.
// This is the one staleness gate shared by both sidebar surfaces (the
// collapsed rail icon and the expanded row) — extracted here so they can't
// drift out of sync.
export function changesCountFor(
  changes: ChangesSlice | null | undefined,
  sessionId: string,
): number | null {
  if (changes?.sessionId !== sessionId || changes.phase !== "loaded") {
    return null
  }
  return changes.staged.length + changes.unstaged.length
}

// Builds the vitals row model for one session. `changesCount` is the changed
// (staged + unstaged) file count for this session, or null when that data
// isn't available (the changed-files store slice only ever holds data for the
// currently SELECTED session — see ChangesSlice in lib/store.ts — so a
// non-selected row omits this line rather than showing a stale/wrong count).
export function buildAgentVitals(
  session: SessionView,
  projectName: string,
  changesCount: number | null,
): AgentVitalsModel {
  const rows: AgentVitalsRow[] = []

  const branch = branchValue(session)
  if (branch) rows.push({ key: "branch", label: "Branch", value: branch, mono: true })

  if (changesCount !== null && changesCount > 0) {
    rows.push({
      key: "changes",
      label: "Changes",
      value: `${changesCount} file${changesCount === 1 ? "" : "s"}`,
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

  if (session.worktree_path) {
    rows.push({
      key: "worktree",
      label: "Worktree",
      value: session.worktree_path,
      mono: true,
    })
  }

  return {
    name: session.title || session.branch_name,
    provider: session.provider,
    statusLabel: vitalsStatusLabel(session),
    statusColorClass: statusDotColorClass(
      session.status,
      session.needs_attention,
    ),
    projectName,
    rows,
  }
}
