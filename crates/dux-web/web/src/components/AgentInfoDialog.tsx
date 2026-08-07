import { TriangleAlert } from "lucide-react"

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
import { branchDrift } from "@/lib/agentTabs"
import { formatDisplayDate } from "@/lib/projectInfo"
import { closeAgentInfo, useDux } from "@/lib/store"
import type { SessionView } from "@/lib/types"

// Friendly label for a session status. The raw value is a lowercase enum
// ("active" | "detached" | "exited"); title-case it for display.
function statusLabel(status: SessionView["status"]): string {
  return status.charAt(0).toUpperCase() + status.slice(1)
}

// Read-only "Agent info…" modal. Pure presentation of existing ViewModel data:
// no wire commands, no git reads. Mirrors `ProjectInfoDialog` (Dialog primitives,
// the `InfoRow` definition list, and the vanished-target guard) so the two info
// surfaces stay consistent. Works identically on desktop and mobile.
export function AgentInfoDialog() {
  const { agentInfoTarget, spine } = useDux()

  // Derive the session from the ViewModel so an agent removed while the dialog
  // is open closes it gracefully, mirroring the project info dialog.
  let session: SessionView | undefined
  if (agentInfoTarget && spine) {
    session = spine.sessions.find((s) => s.id === agentInfoTarget)
  }

  // Closes the dialog when the agent vanishes from the ViewModel; see the hook.
  const isOpen = useVanishedTargetGuard(
    agentInfoTarget !== null,
    session !== undefined,
    closeAgentInfo,
  )

  function handleOpenChange(open: boolean) {
    if (!open) closeAgentInfo()
  }

  // Compute the body only when a session resolves so the hooks above still run
  // unconditionally on every render.
  let body: React.ReactNode = null
  if (session) {
    const name = session.title || session.branch_name
    const project = spine?.projects.find((p) => p.id === session.project_id)
    // The current branch has drifted from the branch the agent was born on. The
    // shared helper flags it only when `initial_branch` is present (older servers
    // omit it) and truly differs.
    const { drifted } = branchDrift(session)
    const tabCount = session.tabs.length
    body = (
      <dl className="flex flex-col gap-3">
        <InfoRow label="Name">{name}</InfoRow>
        <InfoRow label="Provider">{session.provider}</InfoRow>
        {project?.name ? (
          <InfoRow label="Project">{project.name}</InfoRow>
        ) : null}
        <InfoRow label="Current branch">
          <span className="font-mono break-all">{session.branch_name}</span>
        </InfoRow>
        <InfoRow label="Original branch">
          {session.initial_branch ? (
            <SimpleTooltip content="The branch this agent was created on (immutable).">
              <span className="font-mono break-all">
                {session.initial_branch}
              </span>
            </SimpleTooltip>
          ) : (
            <span className="text-muted-foreground">Unknown</span>
          )}
        </InfoRow>
        <InfoRow label="Forked from">
          {session.source_branch ? (
            <SimpleTooltip content="The leading branch this agent was forked from at creation.">
              <span className="font-mono break-all">
                {session.source_branch}
              </span>
            </SimpleTooltip>
          ) : (
            <span className="text-muted-foreground">Unknown</span>
          )}
        </InfoRow>
        {drifted ? (
          // Warning cue next to the branch rows: the working branch no longer
          // matches the branch the agent was created on. Amber + icon to mirror
          // the TUI's warning-toned drift line, so both surfaces flag it equally.
          <p className="flex items-center gap-1.5 text-xs text-amber-500">
            <TriangleAlert className="size-3.5 shrink-0" />
            The branch changed since creation.
          </p>
        ) : null}
        <InfoRow label="Worktree">
          <span className="font-mono break-all">{session.worktree_path}</span>
        </InfoRow>
        <InfoRow label="Status">{statusLabel(session.status)}</InfoRow>
        <InfoRow label="Created">
          {formatDisplayDate(session.created_at)}
        </InfoRow>
        <InfoRow label="Updated">
          {formatDisplayDate(session.updated_at)}
        </InfoRow>
        <InfoRow label="Tabs">
          {tabCount === 1 ? "1 tab" : `${tabCount} tabs`}
        </InfoRow>
        {session.pr ? (
          // Mirrors the TUI Agent Info's "Pull request:" line, including the
          // "manually attached" cue: this row is where a pin says it is one.
          <InfoRow label="Pull request">
            #{session.pr.number} ({session.pr.state}) {session.pr.title}
            {session.pr.overridden ? (
              <span className="text-muted-foreground">
                {" "}
                (manually attached)
              </span>
            ) : null}
          </InfoRow>
        ) : null}
      </dl>
    )
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      {/* Wider than the sm:max-w-sm default: the branch and worktree rows carry
          full paths that deserve room before wrapping. */}
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>
            {session ? session.title || session.branch_name : "Agent info"}
          </DialogTitle>
        </DialogHeader>
        {body}
        <DialogFooter showCloseButton />
      </DialogContent>
    </Dialog>
  )
}
