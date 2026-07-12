import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { closeEditorCloseTab, editorCloseTab, useDux } from "@/lib/store"

// Confirmation before closing a DIRTY editor tab (the per-tab close affordance
// in the strip). A clean tab closes immediately with no dialog, see
// `editorTabs.ts` `shouldConfirmClose`, which the strip consults before
// routing here. Clones `ConfirmDiscardFileDialog`'s structure, the destructive-
// confirm template (CLAUDE.md tenet): Cancel defaults focus, a misclick-safe
// spacer sits above the footer, and the vanished-target guard self-closes the
// dialog if its target tab disappears (closed elsewhere, or the tab stopped
// being dirty because it was saved from another surface).
export function ConfirmCloseEditorTabDialog() {
  const { editorCloseTabTarget, editorTabs } = useDux()

  const tabsState = editorCloseTabTarget
    ? editorTabs[editorCloseTabTarget.sessionId]
    : undefined
  const tab = tabsState?.tabs.find((t) => t.id === editorCloseTabTarget?.tabId)
  // If the tab stopped being dirty (saved elsewhere) the dialog self-closes
  // rather than lingering on a discard prompt with nothing left to discard.
  const present = tab !== undefined && tab.dirty

  const isOpen = useVanishedTargetGuard(
    editorCloseTabTarget !== null,
    present,
    closeEditorCloseTab,
  )
  const path = tab?.path ?? ""

  function handleConfirm() {
    if (!editorCloseTabTarget) return
    editorCloseTab(editorCloseTabTarget.sessionId, editorCloseTabTarget.tabId)
    closeEditorCloseTab()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeEditorCloseTab()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Discard unsaved changes?</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          Your edits to <span className="font-mono">{path}</span> haven&rsquo;t
          been saved. They will be lost.
        </p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          {/* Cancel is the default focus, matching the discard-file dialog.
              shadcn/base-ui buttons already activate on Space/Enter
              natively. */}
          <Button variant="outline" autoFocus onClick={closeEditorCloseTab}>
            Keep editing
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Discard & close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
