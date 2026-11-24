import { FolderPlus, GitBranch, TriangleAlert } from "lucide-react"
import { useMemo, useState } from "react"

import { Modal, useModalLifetime } from "@/components/modal"
import { ProjectRows } from "@/components/ProjectList"
import { useProjectSearch, type ProjectListRow } from "@/components/projectSearch"
import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import { StartTruncatedText } from "@/components/StartTruncatedText"
import { formatRegularCount } from "@/lib/formatRegularCount"
import { PROJECT_FOLDER_MISSING } from "@/lib/projects"
import {
  applyFrozenOrder,
  orderProjectsByRecency,
  projectAgentCounts,
} from "@/lib/projectOrder"
import { closeProjects, openAddProject, useDux } from "@/lib/store"
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
  const life = useModalLifetime(projectsDialogOpen)
  // Keyed by the open, so the frozen order and the search reset on each one.
  return life.mounted ? (
    <ProjectsBody key={life.epoch} open={projectsDialogOpen} onClosed={life.onClosed} />
  ) : null
}

// A row of the list before it is drawn: a live project, or an orphaned group
// (agents whose project record is gone), which offers only Remove project….
type Entry =
  | { id: string; kind: "project"; project: ProjectView }
  | { id: string; kind: "orphan"; group: SidebarGroup }

function ProjectsBody({ open, onClosed }: { open: boolean; onClosed: () => void }) {
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
          {/* The folder's tail is what tells two projects apart, so the path
              gives way at its start and the hover says all of it. */}
          <StartTruncatedText text={project.path} className="font-mono" tooltip />
          {project.path_missing ? <Warning>{PROJECT_FOLDER_MISSING}</Warning> : null}
          <span className="flex shrink-0 items-center gap-1">
            <GitBranch className="size-3 shrink-0" />
            {project.leading_branch ?? "no base yet"}
          </span>
        </DetailLine>
      ),
    }
  })

  // Without a pick, Enter picks nothing and a row's own button opens its menu.
  const search = useProjectSearch(rows, undefined)
  return (
    <Modal
      kind="picker"
      open={open}
      onClose={closeProjects}
      onClosed={onClosed}
      size="lg"
      title="Projects"
      description="Every project dux knows about. Open one to see its actions."
      search={{
        value: search.query,
        onChange: search.setQuery,
        label: "Search projects",
        fieldProps: search.pick.fieldProps,
      }}
      steadyHeight
      doors={[{ label: "Add a new project…", icon: FolderPlus, onSelect: openAddProject }]}
    >
      <ProjectRows
        search={search}
        heading="Choose a project"
        menu={(id) => <ProjectMenuItems id={id} onLeave={closeProjects} />}
      />
    </Modal>
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
    <span className="flex shrink-0 items-center gap-1 text-dux-warning">
      <TriangleAlert className="size-3 shrink-0" />
      {children}
    </span>
  )
}
