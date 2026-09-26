import { GitBranch, TriangleAlert } from "lucide-react"
import { useMemo, useState } from "react"

import { ProjectList, type ProjectListRow } from "@/components/ProjectList"
import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import { Dialog, DialogContent } from "@/components/ui/dialog"
import { formatRegularCount } from "@/lib/formatRegularCount"
import {
  applyFrozenOrder,
  orderProjectsByRecency,
  projectAgentCounts,
} from "@/lib/projectOrder"
import { closeProjects, useDux } from "@/lib/store"
import type { ProjectView, SidebarGroup } from "@/lib/types"

// The Projects list, opened from the app menu: every project and every orphaned
// group, each with its whole project menu. It is where a project with no agents
// is managed, which otherwise only the New agent picker could reach.
//
// Project dialogs (Delete, Remove, Check out default branch, Change base
// branch, Project info, Project settings, Worktrees) open OVER the list, so
// closing one lands back on it; an item that takes the user somewhere else (a
// new agent, a new terminal) closes the list first, through `onLeave`.
export function ProjectsDialog() {
  const { projectsDialogOpen } = useDux()
  return (
    <Dialog
      open={projectsDialogOpen}
      onOpenChange={(open) => {
        if (!open) closeProjects()
      }}
    >
      {/* The New agent picker's layout, for the same reasons: a flex column
          whose list is the one child that shrinks under the soft keyboard. */}
      <DialogContent className="flex flex-col gap-0 p-0 sm:max-w-lg">
        {/* Mounted only while open, so the frozen order and the search reset
            on each open without a reset effect. */}
        {projectsDialogOpen ? <ProjectsBody /> : null}
      </DialogContent>
    </Dialog>
  )
}

// A row of the list before it is drawn: a live project, or an orphaned group
// (agents whose project record is gone), which offers only Remove project….
type Entry =
  | { id: string; kind: "project"; project: ProjectView }
  | { id: string; kind: "orphan"; group: SidebarGroup }

function ProjectsBody() {
  const { spine } = useDux()
  const sessions = useMemo(() => spine?.sessions ?? [], [spine])
  const entries = useMemo<Entry[]>(() => {
    const projects: Entry[] = (spine?.projects ?? []).map((project) => ({
      id: project.id,
      kind: "project",
      project,
    }))
    // Read from the sidebar groups, where the Remove dialog reads an orphan's
    // name too, so the two cannot disagree about what the ghost is called.
    const orphans: Entry[] = (spine?.sidebar.groups ?? [])
      .filter((group) => group.orphaned)
      .map((group) => ({ id: group.project_id, kind: "orphan", group }))
    return [...projects, ...orphans]
  }, [spine])

  // The recency order is snapshotted at open, the way the New agent picker's
  // is, so a spine update cannot slide a row out from under the pointer; the
  // rows themselves stay live, so a removed project disappears. Orphans have no
  // instant of their own and follow the projects.
  const [frozenIds] = useState(() => [
    ...orderProjectsByRecency(spine?.projects ?? [], sessions).map(
      (project) => project.id,
    ),
    ...entries.filter((entry) => entry.kind === "orphan").map((e) => e.id),
  ])
  const ordered = useMemo(
    () => applyFrozenOrder(frozenIds, entries),
    [frozenIds, entries],
  )
  const agentCounts = useMemo(() => projectAgentCounts(sessions), [sessions])

  const rows: ProjectListRow[] = ordered.map((entry) => {
    const label = formatRegularCount(agentCounts.get(entry.id) ?? 0, "agent")
    if (entry.kind === "orphan") {
      return {
        id: entry.id,
        name: entry.group.name,
        label,
        detail: (
          <DetailLine>
            <Warning>Project record is gone</Warning>
          </DetailLine>
        ),
      }
    }
    const { project } = entry
    return {
      id: project.id,
      name: project.name,
      label,
      detail: (
        <DetailLine>
          <span className="min-w-0 truncate font-mono">{project.path}</span>
          {project.path_missing ? <Warning>Folder missing</Warning> : null}
          <span className="flex shrink-0 items-center gap-1">
            <GitBranch className="size-3 shrink-0" />
            {project.leading_branch ?? "no base yet"}
          </span>
        </DetailLine>
      ),
    }
  })

  return (
    <ProjectList
      title="Projects"
      description="Every project dux knows about. Open one to see its actions."
      rows={rows}
      menu={(id) => <ProjectMenuItems id={id} onLeave={closeProjects} />}
    />
  )
}

function DetailLine({ children }: { children: React.ReactNode }) {
  return (
    <span className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
      {children}
    </span>
  )
}

function Warning({ children }: { children: React.ReactNode }) {
  return (
    <span className="flex shrink-0 items-center gap-1 text-amber-500">
      <TriangleAlert className="size-3 shrink-0" />
      {children}
    </span>
  )
}
