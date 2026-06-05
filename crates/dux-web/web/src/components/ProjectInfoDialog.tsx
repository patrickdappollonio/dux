import { useEffect } from "react"

import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { projectBranchDisplay } from "@/lib/projectBranch"
import { formatAddedDate, projectLiveCounts } from "@/lib/projectInfo"
import { closeProjectInfo, useDux } from "@/lib/store"
import type { ProjectView } from "@/lib/types"

// One labelled row in the definition list. The value column is allowed to wrap
// (paths, branch names) so long values stay readable on phones.
function InfoRow({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1 max-sm:grid-cols-1">
      <dt className="text-sm text-muted-foreground">{label}</dt>
      <dd className="min-w-0 text-sm">{children}</dd>
    </div>
  )
}

// Read-only "Project info…" modal. Pure presentation of existing ViewModel data:
// no wire commands, no git reads. Works identically on desktop and mobile.
export function ProjectInfoDialog() {
  const { projectInfoTarget, viewModel } = useDux()

  // Derive the project from the ViewModel so a project removed while the dialog
  // is open closes it gracefully via the effect below, mirroring the terminal
  // confirmation dialog's vanished-target handling.
  let project: ProjectView | undefined
  if (projectInfoTarget && viewModel) {
    project = viewModel.projects.find((p) => p.id === projectInfoTarget)
  }

  // If the target was set but no longer exists in the ViewModel, the project was
  // removed. Drop the modal so it doesn't linger pointing at a gone project.
  useEffect(() => {
    if (projectInfoTarget && !project) {
      closeProjectInfo()
    }
  }, [projectInfoTarget, project])

  const isOpen = projectInfoTarget !== null && project !== undefined

  function handleOpenChange(open: boolean) {
    if (!open) closeProjectInfo()
  }

  // Compute the body only when a project resolves so the hooks above still run
  // unconditionally on every render.
  let body: React.ReactNode = null
  if (project && viewModel) {
    const branch = projectBranchDisplay(project)
    const counts = projectLiveCounts(project.id, viewModel.sessions)
    const envCount = Object.keys(project.env).length
    const providerExplicit = project.explicit_default_provider !== null
    body = (
      <dl className="flex flex-col gap-3">
        <InfoRow label="Path">
          <span className="font-mono break-all">{project.path}</span>
        </InfoRow>
        <InfoRow label="Current branch">
          {branch ? (
            <span
              className={`font-mono ${
                branch.warn ? "text-amber-500" : ""
              }`}
              title={branch.tooltip ?? undefined}
            >
              {branch.branch}
            </span>
          ) : (
            <span className="text-muted-foreground">Unknown</span>
          )}
        </InfoRow>
        <InfoRow label="Default branch">
          {project.leading_branch ? (
            <span className="font-mono">{project.leading_branch}</span>
          ) : (
            <span className="text-muted-foreground">Not detected</span>
          )}
        </InfoRow>
        <InfoRow label="Added">{formatAddedDate(project.created_at)}</InfoRow>
        <InfoRow label="Default provider">
          {project.default_provider}
          {providerExplicit ? (
            <span className="text-muted-foreground"> (explicit)</span>
          ) : null}
        </InfoRow>
        <InfoRow label="Auto-reopen">
          {project.auto_reopen_agents === null
            ? "Inherit"
            : project.auto_reopen_agents
              ? "On"
              : "Off"}
        </InfoRow>
        <InfoRow label="Startup command">
          {project.startup_command ? (
            <span className="font-mono break-all">
              {project.startup_command}
            </span>
          ) : (
            <span className="text-muted-foreground">None</span>
          )}
        </InfoRow>
        <InfoRow label="Environment">
          {envCount === 1 ? "1 variable" : `${envCount} variables`}
        </InfoRow>
        <InfoRow label="Live agents">
          {counts.agents === 1 ? "1 agent" : `${counts.agents} agents`}
        </InfoRow>
        <InfoRow label="Companion terminals">
          {counts.terminals === 1
            ? "1 terminal"
            : `${counts.terminals} terminals`}
        </InfoRow>
      </dl>
    )
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{project?.name ?? "Project info"}</DialogTitle>
        </DialogHeader>
        {body}
        <DialogFooter showCloseButton />
      </DialogContent>
    </Dialog>
  )
}
