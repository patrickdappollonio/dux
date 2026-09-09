// Pure helpers backing the read-only info modals, project and agent. Everything is computed
// from the ViewModel: no wire commands, no git reads.

import { groupTerminalsByOwner } from "./terminals"
import type { SessionView, TerminalView } from "./types"
import { workspaceProjectId } from "@/lib/agentWorkspace"

// Live counts for a project: `agents` is the sessions it owns, `terminals` the companion
// terminals across those sessions plus the project's own terminals.
export interface ProjectLiveCounts {
  agents: number
  terminals: number
}

export function projectLiveCounts(
  projectId: string,
  sessions: SessionView[],
  allTerminals: readonly TerminalView[] = [],
): ProjectLiveCounts {
  const { bySession, byProject } = groupTerminalsByOwner(allTerminals)
  let agents = 0
  let terminals = byProject.get(projectId)?.length ?? 0
  for (const session of sessions) {
    if (workspaceProjectId(session.workspace) !== projectId) continue
    agents += 1
    terminals += bySession.get(session.id)?.length ?? 0
  }
  return { agents, terminals }
}

// Format an RFC 3339 timestamp as a human-readable date, shared by the info modals. An empty
// or unparseable value reads "Unknown", so a modal never renders a raw ISO string.
export function formatDisplayDate(iso: string): string {
  if (iso.trim() === "") return "Unknown"
  const ms = Date.parse(iso)
  if (Number.isNaN(ms)) return "Unknown"
  return new Date(ms).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  })
}
