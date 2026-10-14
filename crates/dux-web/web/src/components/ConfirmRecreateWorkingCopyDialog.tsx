import { useState } from "react"

import {
  AttachedSection,
  GuardedConfirmButton,
} from "@/components/AttachedSection"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useAttachedOverride } from "@/hooks/use-attached-override"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
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
  const isOpen = useVanishedTargetGuard(
    recreateWorkingCopyTarget !== null,
    session !== undefined && managed?.worktree_missing === true,
    closeRecreateWorkingCopy,
  )
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

  function handleOpenChange(next: boolean) {
    if (!next) closeRecreateWorkingCopy()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Recreate working copy?</DialogTitle>
          <DialogDescription className="whitespace-pre-line">
            {managed && view
              ? renderProse(
                  recreateConfirmProse(
                    managed.worktree_label ?? managed.worktree_path,
                    managed.branch_name,
                    managed.source_branch,
                    managed.conversation_resumes === true,
                    view.running,
                  ),
                )
              : ""}
          </DialogDescription>
        </DialogHeader>
        {offer.length > 0 && (
          <div className="flex items-center gap-2">
            <Checkbox
              id="recreate-restart-tabs"
              checked={restart.ticked}
              onCheckedChange={(ticked) =>
                setRestart({ target: recreateWorkingCopyTarget, ticked })
              }
            />
            <label htmlFor="recreate-restart-tabs" className="min-w-0 text-sm">
              {renderProse(recreateRestartCheckboxProse(offer))}
            </label>
          </div>
        )}
        <AttachedSection blockers={shownBlockers} />
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button
            ref={cancelRef}
            variant="outline"
            autoFocus
            onClick={closeRecreateWorkingCopy}
          >
            Cancel
          </Button>
          <GuardedConfirmButton
            verb="Recreate"
            blockers={shownBlockers}
            pending={pending}
            onConfirm={() => void handleConfirm()}
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
