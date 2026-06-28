import type { ProjectView, SessionView, SidebarModel } from "@/lib/types"

// The shape both the desktop sidebar and the mobile home screen render from.
export interface PartitionedProjects {
  // Sessions grouped under their owning project id, in display order.
  grouped: Map<string, SessionView[]>
  // Project ids that have at least one agent (active projects first, with any
  // orphaned project ids — a session whose project is absent — appended so a
  // session is never dropped).
  withAgents: string[]
  // Project ids with no agents, sunk below under their own heading.
  withoutAgents: string[]
  // The drag-reorder payload the server expects: every REAL project id in
  // display order (agent-bearing first, then agent-less), with orphan ids
  // excluded — the server has no project record to reorder for a ghost id.
  realOrder: string[]
  // Resolve a project id to its display name, falling back to a short id slice.
  projectName: (id: string) => string
}

// Project GROUPING is owned by dux_core (`dux_core::sidebar`, surfaced as
// `spine.sidebar`): which sessions belong to which project, which ids are
// orphaned (a session whose project record was removed), the display names, and
// whether the agent-less projects are split below a separator. This function
// only PROJECTS that core model into the shape the components render — it makes
// no grouping decisions of its own. Ordering follows the caller's already
// reordered `projects`/`sessions`, since optimistic drag-reorder is display-only
// state the server has not confirmed yet. Because the TUI consumes the same core
// model, both surfaces group identically by construction.
export function partitionProjects(
  sidebar: SidebarModel | undefined,
  projects: ProjectView[],
  sessions: SessionView[],
): PartitionedProjects {
  const groups = sidebar?.groups ?? []
  const agentlessStart = sidebar?.agentless_start ?? null

  const names = new Map<string, string>()
  const orphanIds: string[] = []
  const agentless = new Set<string>()
  groups.forEach((group, index) => {
    names.set(group.project_id, group.name)
    if (group.orphaned) orphanIds.push(group.project_id)
    if (agentlessStart !== null && index >= agentlessStart) {
      agentless.add(group.project_id)
    }
  })

  // Sessions grouped under their project, in display (reordered) order.
  const grouped = new Map<string, SessionView[]>()
  for (const id of names.keys()) grouped.set(id, [])
  for (const session of sessions) {
    grouped.get(session.project_id)?.push(session)
  }

  // Real projects in display order, partitioned by core's agent-less set; orphan
  // groups (always with agents) appended after the agent-bearing projects.
  const withAgents: string[] = []
  const withoutAgents: string[] = []
  for (const project of projects) {
    if (!names.has(project.id)) continue
    if (agentless.has(project.id)) withoutAgents.push(project.id)
    else withAgents.push(project.id)
  }
  withAgents.push(...orphanIds)

  // Order for the reorder payload: real ids only (orphans were appended to
  // withAgents above; the server rejects ids it has no project record for).
  const orphanSet = new Set(orphanIds)
  const realOrder = [...withAgents, ...withoutAgents].filter(
    (id) => !orphanSet.has(id),
  )

  const projectName = (id: string) => names.get(id) ?? id.slice(0, 8)
  return { grouped, withAgents, withoutAgents, realOrder, projectName }
}
