// The New dialog's "New terminal for an agent" step: which agents it lists and
// under which headings. Every agent is offered whatever it is doing, a detached
// or stopped one included, because the terminal opens in its directory and
// needs no live provider.

import { sessionLabel, workspaceProjectId } from "@/lib/agentWorkspace"
import { partitionProjects } from "@/lib/projects"
import type { ProjectView, SessionView, SidebarModel } from "@/lib/types"

export interface AgentTerminalGroup {
  /** A project id, or "standalone" for the agents that belong to none. */
  id: string
  title: string
  sessions: SessionView[]
}

/**
 * The agents matching `query` by name, grouped under their projects in the
 * sidebar's project order, then the standalone agents under their own
 * heading. A group the query empties is left out.
 */
export function agentTerminalGroups(
  spine: {
    sidebar?: SidebarModel
    projects: ProjectView[]
    sessions: SessionView[]
  },
  query: string,
): AgentTerminalGroup[] {
  const needle = query.trim().toLowerCase()
  const matches = (session: SessionView) =>
    needle === "" || sessionLabel(session).toLowerCase().includes(needle)
  const { withAgents, grouped, projectName } = partitionProjects(
    spine.sidebar,
    spine.projects,
    spine.sessions,
  )
  const groups: AgentTerminalGroup[] = withAgents.map((id) => ({
    id,
    title: projectName(id),
    sessions: (grouped.get(id) ?? []).filter(matches),
  }))
  groups.push({
    id: "standalone",
    title: "Standalone",
    sessions: spine.sessions.filter(
      (session) =>
        workspaceProjectId(session.workspace) === null && matches(session),
    ),
  })
  return groups.filter((group) => group.sessions.length > 0)
}
