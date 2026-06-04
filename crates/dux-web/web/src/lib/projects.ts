import type { ProjectView, SessionView } from "@/lib/types"

// The shape both the desktop sidebar and the mobile home screen render from.
export interface PartitionedProjects {
  // Sessions grouped under their owning project id.
  grouped: Map<string, SessionView[]>
  // Project ids that have at least one agent (active projects first, with any
  // orphaned project ids — a session whose project is absent — appended so a
  // session is never dropped).
  withAgents: string[]
  // Project ids with no agents, sunk below under their own heading.
  withoutAgents: string[]
  // Resolve a project id to its display name, falling back to a short id slice.
  projectName: (id: string) => string
}

// Group sessions under EVERY project, then partition like the TUI: projects
// with agents first, agent-less ones under their own "Projects with no agents"
// heading. Shared verbatim by the sidebar and the mobile home screen so both
// surfaces order projects identically.
export function partitionProjects(
  projects: ProjectView[],
  sessions: SessionView[],
): PartitionedProjects {
  const grouped = new Map<string, SessionView[]>()
  for (const project of projects) {
    grouped.set(project.id, [])
  }
  const orphanIds: string[] = []
  for (const session of sessions) {
    let bucket = grouped.get(session.project_id)
    if (!bucket) {
      bucket = []
      grouped.set(session.project_id, bucket)
      orphanIds.push(session.project_id)
    }
    bucket.push(session)
  }
  const withAgents: string[] = []
  const withoutAgents: string[] = []
  for (const project of projects) {
    if ((grouped.get(project.id)?.length ?? 0) > 0) {
      withAgents.push(project.id)
    } else {
      withoutAgents.push(project.id)
    }
  }
  withAgents.push(...orphanIds)

  const projectName = (id: string) =>
    projects.find((p) => p.id === id)?.name ?? id.slice(0, 8)

  return { grouped, withAgents, withoutAgents, projectName }
}
