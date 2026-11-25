import { useEffect, useState } from "react"

import { CheckboxRow } from "@/components/CheckboxRow"
import { ErrorLine } from "@/components/ErrorLine"
import { Modal, useHeldWhileClosed } from "@/components/modal"
import { AgentNameInput } from "@/components/AgentNameFields"
import { FormGroup } from "@/components/ui/form-group"
import { INVALID_AGENT_NAME, isValidAgentName } from "@/lib/agentName"
import { matchWorkspace, standaloneAgentDefaultName } from "@/lib/agentWorkspace"
import { renderProse } from "@/lib/prose"
import {
  renameBlockedReason,
  renameBranchNoteProse,
} from "@/lib/renameAgent"
import { sessionsApi } from "@/lib/sessionsApi"
import {
  closeRename,
  setRenameBranch,
  setRenameDraft,
  submitRename,
  useDux,
} from "@/lib/store"

// Rename an agent: the same dialog the terminal UI shows. The field pre-fills the
// current custom name (empty when none), with the branch name (a standalone
// agent's folder name, converted into a valid agent name) as the placeholder,
// because an empty name goes back to it. Every agent's name follows the same
// rules, a standalone agent's included. An agent with a branch also gets an unticked "Also rename the git branch"
// box, with a line saying what stays on the old name when the branch has an
// open or draft pull request or a pushed copy.
export function RenameSessionDialog() {
  const live = useDux()
  const isOpen = live.renameTarget !== null
  // What the dialog showed when the close began, kept while the close animates.
  const renameTarget = useHeldWhileClosed(isOpen, live.renameTarget)
  const renameDraft = useHeldWhileClosed(isOpen, live.renameDraft)
  const renameBranch = useHeldWhileClosed(isOpen, live.renameBranch)
  const renamePending = useHeldWhileClosed(isOpen, live.renamePending)
  const spine = useHeldWhileClosed(isOpen, live.spine)
  const session = spine?.sessions.find((s) => s.id === renameTarget)
  const branch = session
    ? matchWorkspace<string | null>(session.workspace, {
        managed: (w) => w.branch_name,
        folder: () => null,
      })
    : null
  const fallbackName = session
    ? matchWorkspace(session.workspace, {
        managed: (w) => w.branch_name,
        folder: (w) => standaloneAgentDefaultName(w.folder_path),
      })
    : ""
  const branchNamed = branch !== null
  // The open or draft pull request that keeps the old name, as the engine
  // answers it for every surface.
  const openPr = branch !== null ? (session?.rename_warning_pr ?? null) : null

  // Whether the branch has a pushed copy, tagged with the agent it was asked
  // about so an answer that lands after the dialog moved on is ignored.
  const [pushedAnswer, setPushedAnswer] = useState<{
    id: string
    pushed: boolean
  } | null>(null)
  const pushed = pushedAnswer?.id === renameTarget && pushedAnswer.pushed

  const open = renameTarget !== null && session !== undefined

  useEffect(() => {
    if (!open || !branchNamed || !renameTarget) return
    let live = true
    const id = renameTarget
    sessionsApi
      .branchPushed(id)
      .then((answer) => {
        if (live) setPushedAnswer({ id, pushed: answer.pushed === true })
      })
      // No answer is no warning: the box still says what it does.
      .catch(() => {})
    return () => {
      live = false
    }
  }, [open, branchNamed, renameTarget])

  const invalidNonEmpty =
    renameDraft.trim() !== "" &&
    !isValidAgentName(renameDraft.trim())
  const blocked = renameBlockedReason(renameDraft, renameBranch, branchNamed)
  const canSave = !invalidNonEmpty && blocked === null && !renamePending
  const saveRefusal = invalidNonEmpty ? INVALID_AGENT_NAME : blocked
  const warns = openPr !== null || pushed

  function handleSubmit() {
    if (!canSave) return
    void submitRename()
  }

  return (
    <Modal
      kind="form"
      open={isOpen}
      // An agent removed while the dialog is open closes it.
      vanished={isOpen && session === undefined}
      onClose={closeRename}
      row={renameTarget === null ? null : { sessionId: renameTarget }}
      title="Rename agent"
      description={
        branchNamed
          ? "Sets the name this agent shows. Leave it empty to show its branch name again."
          : "Sets the name this agent shows. Leave it empty to show its folder's name again."
      }
      submit={{
        label: "Save",
        tone: "primary",
        onSelect: handleSubmit,
        busy: renamePending,
        disabledReason: saveRefusal,
      }}
    >
      <FormGroup
        layout="stacked"
        control="single"
        spokenName="Agent name"
        help="Any name git accepts for a branch."
        invalid={invalidNonEmpty}
      >
        {(field) => (
          <AgentNameInput
            field={field}
            value={renameDraft}
            onChange={setRenameDraft}
            onSubmit={handleSubmit}
            placeholder={fallbackName}
            generating={false}
            autoFocus={false}
          />
        )}
      </FormGroup>
      {branch !== null && (
        <CheckboxRow
          id="rename-branch"
          checked={renameBranch}
          onCheckedChange={(checked) => setRenameBranch(checked)}
          help={renderProse(renameBranchNoteProse(branch, openPr, pushed))}
          helpTone={warns ? "warning" : undefined}
        >
          Also rename the git branch
        </CheckboxRow>
      )}
      <ErrorLine size="xs">{blocked}</ErrorLine>
    </Modal>
  )
}
