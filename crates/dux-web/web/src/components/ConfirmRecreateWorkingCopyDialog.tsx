import { useState } from "react"

import { AttachedSection } from "@/components/AttachedSection"
import { guardedConfirmAction } from "@/components/guardedConfirmAction"
import { CheckboxRow } from "@/components/CheckboxRow"
import { Modal } from "@/components/modal"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { managedWorkspace } from "@/lib/agentWorkspace"
import { renderProse } from "@/lib/prose"
import {
  recreateConfirmProse,
  recreateRestartCheckboxProse,
  recreateView,
} from "@/lib/recreateWorkingCopy"
import {
  closeRecreateWorkingCopy,
  recreateWorkingCopy,
  useDux,
} from "@/lib/store"

// The confirmation behind putting back a working copy an agent deleted from
// under itself.
//
// Destructive about what was in that directory rather than about the agent: the
// directory comes back, its contents do not. The body says so, and says what a
// tab still running in the deleted directory makes of the recreated one, which
// is the provider's own answer rather than dux's. When a running tab cannot
// follow the folder but can pick its conversation back up, a checkbox, ticked
// by default, restarts it once the copy is back; that stops the tab, so it is
// guarded like any stop.
export function ConfirmRecreateWorkingCopyDialog() {
  const { recreateWorkingCopyTarget, spine } = useDux()

  const session = recreateWorkingCopyTarget
    ? spine?.sessions.find((s) => s.id === recreateWorkingCopyTarget)
    : undefined
  const managed = session ? managedWorkspace(session.workspace) : null
  const view = managed && session ? recreateView(managed, session.provider) : null
  const offer = view?.restart_offer ?? []

  // Closes itself when the agent leaves the live view model, like every other
  // target-keyed dialog. A working copy that came back on its own counts: the
  // menu entry is gone by then and the dialog behind it has nothing to do.
  const present = session !== undefined && managed?.worktree_missing === true
  const isOpen = recreateWorkingCopyTarget !== null && present
  const { blockers, pending, cancelRef, confirm } = useAttachedOverride(
    isOpen,
    recreateWorkingCopyTarget,
  )

  // Ticked afresh for every opening: a tab that cannot follow the folder is
  // stuck until it is restarted.
  const [restart, setRestart] = useState({
    target: recreateWorkingCopyTarget,
    ticked: true,
  })
  if (restart.target !== recreateWorkingCopyTarget) {
    setRestart({ target: recreateWorkingCopyTarget, ticked: true })
  }
  const restartTabs = offer.length > 0 && restart.ticked
  // Only a restart stops anything, so only it can be refused for somebody.
  const shownBlockers = restartTabs ? blockers : null

  async function handleConfirm() {
    if (!recreateWorkingCopyTarget) return
    const target = recreateWorkingCopyTarget
    const done = await confirm((accepted) =>
      recreateWorkingCopy(target, restartTabs, restartTabs ? accepted : null),
    )
    if (done) closeRecreateWorkingCopy()
  }

  return (
    <Modal
      kind="confirm"
      open={recreateWorkingCopyTarget !== null}
      vanished={!present}
      onClose={closeRecreateWorkingCopy}
      title="Recreate working copy?"
      description={
        managed && view ? (
          <span className="whitespace-pre-line">
            {renderProse(
              recreateConfirmProse(
                managed.worktree_label ?? managed.worktree_path,
                managed.branch_name,
                managed.source_branch,
                managed.conversation_resumes === true,
                view.running,
              ),
            )}
          </span>
        ) : null
      }
      cancelLabel="Cancel"
      safeRef={cancelRef}
      confirm={guardedConfirmAction({
        verb: "Recreate",
        blockers: shownBlockers,
        pending,
        onConfirm: () => void handleConfirm(),
      })}
    >
      {offer.length > 0 || shownBlockers?.length ? (
        <>
          {offer.length > 0 && (
            <CheckboxRow
              id="recreate-restart-tabs"
              checked={restart.ticked}
              onCheckedChange={(ticked) =>
                setRestart({ target: recreateWorkingCopyTarget, ticked })
              }
            >
              {renderProse(recreateRestartCheckboxProse(offer))}
            </CheckboxRow>
          )}
          <AttachedSection blockers={shownBlockers} />
        </>
      ) : null}
    </Modal>
  )
}
