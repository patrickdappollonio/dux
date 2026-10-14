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
import { InlineCode } from "@/components/ui/inline-code"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { formatRegularCount } from "@/lib/formatRegularCount"
import { formatDisplayDate } from "@/lib/projectInfo"
import { closeAgentInfo, useDux } from "@/lib/store"
import type { SessionView } from "@/lib/types"
import {
  type AgentWorkspaceWire,
  branchDriftOf,
  matchWorkspace,
  missingDirectoryInfoLine,
  sessionLabel,
  workspaceProjectId,
} from "@/lib/agentWorkspace"

// Friendly label for a session status. The raw value is a lowercase enum
// ("active" | "detached" | "exited"); title-case it for display.
function statusLabel(status: SessionView["status"]): string {
  return status.charAt(0).toUpperCase() + status.slice(1)
}

// The directory this agent runs in is gone. Same amber warning treatment as the
// branch-drift line beside it, and the same words the terminal UI's info panel
// uses, because it is the same fact about the same agent.
function MissingDirectoryLine({
  workspace,
}: {
  workspace: AgentWorkspaceWire
}) {
  const line = missingDirectoryInfoLine(workspace)
  if (line === null) return null
  return (
    <p className="flex items-center gap-1.5 text-xs text-dux-warning">
      <TriangleAlert className="size-3.5 shrink-0" />
      {line}
    </p>
  )
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
    const name = sessionLabel(session)
    const project = spine?.projects.find(
      (p) => p.id === workspaceProjectId(session.workspace),
    )
    // The current branch has drifted from the branch the agent was born on. The
    // shared helper flags it only when `initial_branch` is present (older servers
    // omit it) and truly differs.
    const { drifted } = branchDriftOf(session.workspace)
    const tabCount = session.tabs.length
    body = (
      <dl className="flex flex-col gap-3">
        <InfoRow label="Name">
          <InlineCode>{name}</InlineCode>
        </InfoRow>
        <InfoRow label="Provider">
          <InlineCode>{session.provider}</InlineCode>
        </InfoRow>
        {project?.name ? (
          <InfoRow label="Project">
            <InlineCode>{project.name}</InlineCode>
          </InfoRow>
        ) : null}
        {/* The branch rows exist only for a managed agent. A standalone agent
            gets the one thing that is true of it instead: what it is and where
            it runs. Rendering "Current branch" with nothing after it would be
            worse than no row. Matched exhaustively, so a third kind of
            workspace cannot silently fall into either shape. */}
        {matchWorkspace(session.workspace, {
          managed: (workspace) => (
            <>
              <InfoRow label="Current branch">
                <InlineCode>{workspace.branch_name}</InlineCode>
              </InfoRow>
              <InfoRow label="Original branch">
                {workspace.initial_branch ? (
                  <SimpleTooltip content="The branch this agent was created on (immutable).">
                    <InlineCode>{workspace.initial_branch}</InlineCode>
                  </SimpleTooltip>
                ) : (
                  <span className="text-muted-foreground">Unknown</span>
                )}
              </InfoRow>
              <InfoRow label="Forked from">
                {workspace.source_branch ? (
                  <SimpleTooltip content="The leading branch this agent was forked from at creation.">
                    <InlineCode>{workspace.source_branch}</InlineCode>
                  </SimpleTooltip>
                ) : (
                  <span className="text-muted-foreground">Unknown</span>
                )}
              </InfoRow>
              {drifted ? (
                // Warning cue next to the branch rows: the working branch no
                // longer matches the branch the agent was created on. Amber +
                // icon to mirror the TUI's warning-toned drift line, so both
                // surfaces flag it equally.
                <p className="flex items-center gap-1.5 text-xs text-dux-warning">
                  <TriangleAlert className="size-3.5 shrink-0" />
                  The branch changed since creation.
                </p>
              ) : null}
              <InfoRow label="Worktree">
                <InlineCode>{workspace.worktree_path}</InlineCode>
              </InfoRow>
              <MissingDirectoryLine workspace={workspace} />
            </>
          ),
          folder: (workspace) => (
            <>
              <InfoRow label="Kind">Standalone agent</InfoRow>
              <InfoRow label="Folder">
                <SimpleTooltip content="The folder you pointed this agent at. dux runs the provider here and never creates, moves or removes it.">
                  <InlineCode>{workspace.folder_label}</InlineCode>
                </SimpleTooltip>
              </InfoRow>
              <MissingDirectoryLine workspace={workspace} />
            </>
          ),
        })}
        <InfoRow label="Status">{statusLabel(session.status)}</InfoRow>
        <InfoRow label="Created">
          {formatDisplayDate(session.created_at)}
        </InfoRow>
        <InfoRow label="Updated">
          {formatDisplayDate(session.updated_at)}
        </InfoRow>
        <InfoRow label="Tabs">
          {formatRegularCount(tabCount, "tab")}
        </InfoRow>
        {session.pr ? (
          // Mirrors the TUI Agent Info's "Pull request:" line, including the
          // "manually attached" cue: this row is where a pin says it is one.
          <InfoRow label="Pull request">
            <InlineCode>#{session.pr.number}</InlineCode> ({session.pr.state}){" "}
            {session.pr.title}
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
            {session ? sessionLabel(session) : "Agent info"}
          </DialogTitle>
        </DialogHeader>
        {body}
        <DialogFooter showCloseButton />
      </DialogContent>
    </Dialog>
  )
}
