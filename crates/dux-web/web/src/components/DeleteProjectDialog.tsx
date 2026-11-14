import {
  AttachedSection,
  GuardedConfirmButton,
} from "@/components/AttachedSection"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useEffect, useRef, useState } from "react"
import { WorktreeFilesNote } from "@/components/WorktreeFilesNote"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { closeDeleteProject, deleteProject, useDux } from "@/lib/store"
import { sessionLabel, workspaceProjectId } from "@/lib/agentWorkspace"
import {
  type UncommittedAnswer,
  uncommittedAnswer,
  worktreeFilesNote,
} from "@/lib/deleteAgent"
import { sessionsApi } from "@/lib/sessionsApi"
import { deleteProjectProse } from "@/lib/projectConfirm"
import { renderProse } from "@/lib/prose"

// One line per agent worktree the cascade removes that holds uncommitted
// files, in the agent delete dialog's words: quietly while git is asked, then
// the count or the failed look; a clean worktree says nothing. Mounted only
// while the dialog is open, so every open asks afresh. Each agent is asked
// through its own read-only route, the one the command line's `projects rm`
// asks, so a project-level route would only repeat it; an agent that joins
// the project while the dialog is open is asked too. Nothing here holds the
// confirm.
function ProjectWorktreeFiles({
  agents,
}: {
  agents: readonly { id: string; name: string }[]
}) {
  const [answers, setAnswers] = useState<
    ReadonlyMap<string, UncommittedAnswer>
  >(() => new Map())
  const asked = useRef(new Set<string>())
  const live = useRef(true)
  useEffect(() => {
    live.current = true
    return () => {
      live.current = false
    }
  }, [])
  const ids = agents.map((agent) => agent.id).join("\n")
  useEffect(() => {
    for (const id of ids.split("\n")) {
      if (id === "" || asked.current.has(id)) continue
      asked.current.add(id)
      sessionsApi
        .uncommittedFiles(id)
        .then(
          (wire) =>
            uncommittedAnswer(wire.uncommitted_files, wire.uncommitted_files_error),
          (e: unknown): UncommittedAnswer => ({
            failed: e instanceof Error ? e.message : String(e),
          }),
        )
        .then((answer) => {
          if (live.current) setAnswers((prev) => new Map(prev).set(id, answer))
        })
    }
  }, [ids])
  const notes = agents.flatMap((agent) => {
    const note = worktreeFilesNote(agent.name, answers.get(agent.id) ?? null)
    return note === null ? [] : [{ id: agent.id, note }]
  })
  if (notes.length === 0) return null
  return (
    <div className="flex flex-col gap-1">
      {notes.map(({ id, note }) => (
        <WorktreeFilesNote key={id} note={note} />
      ))}
    </div>
  )
}

// The destructive cascade counterpart to `RemoveProjectDialog`: it also deletes
// every agent's worktree from disk, so the copy spells that out. Offered only for
// a real project, so its open state goes through the vanish guard and the dialog
// closes itself rather than acting on a target the ViewModel no longer has.
export function DeleteProjectDialog() {
  const { deleteProjectTarget, spine } = useDux()

  const project = spine?.projects.find((p) => p.id === deleteProjectTarget)
  const isOpen = useVanishedTargetGuard(
    deleteProjectTarget !== null,
    project !== undefined,
    closeDeleteProject,
  )
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    deleteProjectTarget,
  )
  const name = project?.name
  const projectSessions =
    spine?.sessions.filter(
      (s) => workspaceProjectId(s.workspace) === deleteProjectTarget,
    ) ?? []
  const agentCount = projectSessions.length
  // A project's agents are all managed, and every worktree goes with it.
  const worktrees = projectSessions.map((s) => ({
    id: s.id,
    name: sessionLabel(s),
  }))

  async function handleConfirm() {
    if (!deleteProjectTarget) return
    const target = deleteProjectTarget
    const done = await confirm((accepted) => deleteProject(target, accepted))
    if (done) closeDeleteProject()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeDeleteProject()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Delete project?</DialogTitle>
          <DialogDescription>
            {renderProse(deleteProjectProse(name, agentCount))}
          </DialogDescription>
        </DialogHeader>
        {isOpen && <ProjectWorktreeFiles agents={worktrees} />}
        <AttachedSection blockers={blockers} />
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button
            ref={cancelRef}
            variant="outline"
            autoFocus
            onClick={closeDeleteProject}
          >
            Cancel
          </Button>
          <GuardedConfirmButton
            verb="Delete"
            blockers={blockers}
            pending={pending}
            onConfirm={() => void handleConfirm()}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
