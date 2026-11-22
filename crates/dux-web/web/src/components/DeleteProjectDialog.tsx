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

// A note per worktree the cascade removes with uncommitted files (quiet while git
// is asked, nothing when clean), asked afresh on every opening; it never holds the confirm.
function useProjectWorktreeNotes(
  open: boolean,
  agents: readonly { id: string; name: string }[],
) {
  // Every opening and closing moves `opening`, so answers to an earlier one are
  // never shown in a later one.
  const [seen, setSeen] = useState({ open, opening: 0 })
  if (seen.open !== open) setSeen({ open, opening: seen.opening + 1 })
  const opening = seen.opening
  const [answers, setAnswers] = useState<{
    opening: number
    byId: ReadonlyMap<string, UncommittedAnswer>
  }>(() => ({ opening, byId: new Map() }))
  const asked = useRef({ opening, ids: new Set<string>() })
  const live = useRef(true)
  useEffect(() => {
    live.current = true
    return () => {
      live.current = false
    }
  }, [])
  const ids = open ? agents.map((agent) => agent.id).join("\n") : ""
  useEffect(() => {
    if (asked.current.opening !== opening) asked.current = { opening, ids: new Set() }
    for (const id of ids.split("\n")) {
      if (id === "" || asked.current.ids.has(id)) continue
      asked.current.ids.add(id)
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
          if (!live.current) return
          setAnswers((prev) =>
            prev.opening > opening
              ? prev
              : {
                  opening,
                  byId: new Map(prev.opening === opening ? prev.byId : []).set(id, answer),
                },
          )
        })
    }
  }, [ids, opening])
  const byId = answers.opening === opening ? answers.byId : new Map<string, UncommittedAnswer>()
  if (!open) return []
  return agents.flatMap((agent) => {
    const note = worktreeFilesNote(agent.name, byId.get(agent.id) ?? null)
    return note === null ? [] : [{ id: agent.id, note }]
  })
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
  const notes = useProjectWorktreeNotes(isOpen, worktrees)

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
      {notes.length > 0 || driven.length > 0 || blockers?.length ? (
        <>
          {notes.length > 0 && (
            <div className="flex flex-col gap-1">
              {notes.map(({ id, note }) => (
                <WorktreeFilesNote key={id} note={note} />
              ))}
            </div>
          )}
          <DrivenElsewhereSection driven={driven} />
          <AttachedSection blockers={blockers} />
        </>
      ) : null}
    </Modal>
  )
}
