import { useEffect } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { closeCloseTab, closeTab, useDux } from "@/lib/store"

// Confirmation before closing a tab. Closing ALWAYS confirms (matching the TUI):
// all tabs are generic, so the copy is uniform. dux can't reopen a tab's exact
// conversation; if it's the agent's only remaining tab, closing it detaches the
// agent (which stays in Projects, reopenable). The ✕ on a tab pill opens this
// instead of closing on a single click. Cancel is the default focus.
export function ConfirmCloseTabDialog() {
  const { closeTabTarget, spine } = useDux()

  const session = closeTabTarget
    ? spine?.sessions.find((s) => s.id === closeTabTarget.sessionId)
    : undefined
  const tab = closeTabTarget
    ? session?.tabs.find((t) => t.id === closeTabTarget.tabId)
    : undefined
  const provider = tab?.provider
  const sessionLabel = provider ? `the ${provider} session` : "the session"
  // Closing detaches the agent when it removes the agent's LAST live tab. Count
  // by liveness (dormant siblings from a restart don't keep the agent running),
  // not by total tab count.
  const liveTabs = session?.tabs.filter((t) => t.has_live_process).length ?? 0
  const willDetach = (tab?.has_live_process ?? false)
    ? liveTabs <= 1
    : liveTabs === 0

  // If the tab (or its whole session) vanishes from the ViewModel while the
  // dialog is open (deleted from another client, or the tab already closed),
  // drop the modal instead of lingering on a stale target — the same
  // vanished-target guard the sibling confirm dialogs use.
  useEffect(() => {
    if (closeTabTarget && !tab) {
      closeCloseTab()
    }
  }, [closeTabTarget, tab])

  const isOpen = closeTabTarget !== null && tab !== undefined

  function handleConfirm() {
    if (!closeTabTarget) return
    closeTab(closeTabTarget.sessionId, closeTabTarget.tabId)
    closeCloseTab()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeCloseTab()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Close tab?</DialogTitle>
          <DialogDescription>
            {`This ends ${sessionLabel} in this tab. dux can't reopen this exact conversation — a recent one can be recovered from a fresh tab via your provider's own history command.`}
            {willDetach
              ? " It's this agent's last live tab, so the agent detaches and stays in Projects, reopenable."
              : ""}
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          {/* Cancel is the default focus, matching the TUI. shadcn/radix buttons
              activate on Space/Enter natively. */}
          <Button variant="outline" autoFocus onClick={closeCloseTab}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Close tab
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
