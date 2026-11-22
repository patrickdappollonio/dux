import {
  AttachedSection,
  DrivenElsewhereSection,
} from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import {
  drivenElsewhereHint,
  useProjectDrivenElsewhere,
} from "@/hooks/use-agent-active-elsewhere"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { Modal } from "@/components/modal"
import { useEffect, useRef, useState } from "react"
import { WorktreeFilesNote } from "@/components/WorktreeFilesNote"
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
  const present = project !== undefined
  const isOpen = deleteProjectTarget !== null && present
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    deleteProjectTarget,
  )
  const driven = useProjectDrivenElsewhere(deleteProjectTarget)
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

  return (
    <Modal
      kind="confirm"
      open={deleteProjectTarget !== null}
      vanished={!present}
      onClose={closeDeleteProject}
      title="Delete project?"
      description={renderProse(deleteProjectProse(name, agentCount))}
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Delete",
        drivenHint: drivenElsewhereHint(driven),
        blockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {worktrees.length > 0 || driven.length > 0 || blockers?.length ? (
        <>
          {isOpen && <ProjectWorktreeFiles agents={worktrees} />}
          <DrivenElsewhereSection driven={driven} />
          <AttachedSection blockers={blockers} />
        </>
      ) : null}
    </Modal>
  )
}
