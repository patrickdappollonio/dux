import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { DOCS_AGENT_TABS_CLOSING } from "@/lib/docs"
import { closeCloseTab, closeTab, useDux } from "@/lib/store"

// Confirmation before closing a tab. Only an EXTRA tab can reach this dialog:
// the agent's first tab cannot be closed at all, its tab-strip menu item is
// disabled, and the Task Manager's row for it stops the agent through
// ConfirmStopAgentDialog instead. So the copy here is unconditionally the
// destructive one. Closing always confirms (matching the TUI). dux can't reopen
// a tab's exact conversation; if this was the agent's last live tab, closing it
// detaches the agent (which stays in Projects, reopenable). Cancel is the
// default focus.
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

  // Closes the dialog when the tab (or its whole session) vanishes from the
  // ViewModel; see the hook.
  const isOpen = useVanishedTargetGuard(
    closeTabTarget !== null,
    tab !== undefined,
    closeCloseTab,
  )

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
            {`This ends ${sessionLabel} in this tab and deletes the tab for good. A new tab always starts fresh, so use your provider's own history command to get back to this conversation.`}
            {willDetach
              ? " It's this agent's last live tab, so the agent detaches and stays in Projects, reopenable."
              : ""}{" "}
            <a
              href={DOCS_AGENT_TABS_CLOSING}
              target="_blank"
              rel="noopener noreferrer"
              className="text-primary underline underline-offset-2"
            >
              How closing a tab works →
            </a>
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
