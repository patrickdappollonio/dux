import { InfoRow } from "@/components/InfoRow"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { formatRegularCount } from "@/lib/formatRegularCount"
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
    const counts = projectLiveCounts(project.id, spine.sessions, spine.terminals)
    const envCount = Object.keys(project.env).length
    const providerExplicit = project.explicit_default_provider !== null
    body = (
      <dl className="flex flex-col gap-3">
        <InfoRow label="Path">
          <InlineCode>{project.path}</InlineCode>
        </InfoRow>
        <InfoRow label="Current branch">
          {branch ? (
            <SimpleTooltip content={branch.tooltip ?? undefined}>
              <InlineCode className={branch.warn ? "text-amber-500" : undefined}>
                {branch.branch}
              </InlineCode>
            </SimpleTooltip>
          ) : (
            <span className="text-muted-foreground">Unknown</span>
          )}
        </InfoRow>
        <InfoRow label="Base branch">
          {project.leading_branch ? (
            <InlineCode>{project.leading_branch}</InlineCode>
          ) : (
            <span className="text-muted-foreground">No base recorded yet</span>
          )}
        </InfoRow>
        <InfoRow label="Added">{formatDisplayDate(project.created_at)}</InfoRow>
        <InfoRow label="Default provider">
          <InlineCode>{project.default_provider}</InlineCode>
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
            <InlineCode>{project.startup_command}</InlineCode>
          ) : (
            <span className="text-muted-foreground">None</span>
          )}
        </InfoRow>
        <InfoRow label="Environment">
          {formatRegularCount(envCount, "variable")}
        </InfoRow>
        <InfoRow label="Live agents">
          {formatRegularCount(counts.agents, "agent")}
        </InfoRow>
        <InfoRow label="Companion terminals">
          {formatRegularCount(counts.terminals, "terminal")}
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
