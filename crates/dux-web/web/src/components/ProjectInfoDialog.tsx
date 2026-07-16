import { InfoRow } from "@/components/InfoRow"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { projectBranchDisplay } from "@/lib/projectBranch"
import { formatDisplayDate, projectLiveCounts } from "@/lib/projectInfo"
import { closeProjectInfo, useDux } from "@/lib/store"
import type { ProjectView } from "@/lib/types"

// Read-only "Project info…" modal. Pure presentation of existing ViewModel data:
// no wire commands, no git reads. Works identically on desktop and mobile.
export function ProjectInfoDialog() {
  const { projectInfoTarget, spine } = useDux()

  // Derive the project from the ViewModel so a project removed while the dialog
  // is open closes it gracefully, mirroring the terminal confirmation dialog.
  let project: ProjectView | undefined
  if (projectInfoTarget && spine) {
    project = spine.projects.find((p) => p.id === projectInfoTarget)
  }

  // Closes the dialog when the project vanishes from the ViewModel; see the hook.
  const isOpen = useVanishedTargetGuard(
    projectInfoTarget !== null,
    project !== undefined,
    closeProjectInfo,
  )

  function handleOpenChange(open: boolean) {
    if (!open) closeProjectInfo()
  }

  // Compute the body only when a project resolves so the hooks above still run
  // unconditionally on every render.
  let body: React.ReactNode = null
  if (project && spine) {
    const branch = projectBranchDisplay(project)
    const counts = projectLiveCounts(
      project.id,
      spine.sessions,
      project.terminals,
    )
    const envCount = Object.keys(project.env).length
    const providerExplicit = project.explicit_default_provider !== null
    body = (
      <dl className="flex flex-col gap-3">
        <InfoRow label="Path">
          <span className="font-mono break-all">{project.path}</span>
        </InfoRow>
        <InfoRow label="Current branch">
          {branch ? (
            <SimpleTooltip content={branch.tooltip ?? undefined}>
              <span
                className={`font-mono ${
                  branch.warn ? "text-amber-500" : ""
                }`}
              >
                {branch.branch}
              </span>
            </SimpleTooltip>
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
        <InfoRow label="Added">{formatDisplayDate(project.created_at)}</InfoRow>
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
