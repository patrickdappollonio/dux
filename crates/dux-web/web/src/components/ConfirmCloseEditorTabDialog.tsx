import { Modal } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { closeEditorCloseTab, editorCloseTab, useDux } from "@/lib/store"
import { rootKey } from "@/lib/editorRoot"

// Confirmation before closing a DIRTY editor tab; a clean one closes with no
// dialog (`shouldConfirmClose`). Follows the destructive-confirm template, and the
// vanished-target guard self-closes it if the tab is closed or saved elsewhere.
export function ConfirmCloseEditorTabDialog() {
  const { editorCloseTabTarget, editorTabs } = useDux()

  const tabsState = editorCloseTabTarget
    ? editorTabs[rootKey(editorCloseTabTarget.root)]
    : undefined
  const tab = tabsState?.tabs.find((t) => t.id === editorCloseTabTarget?.tabId)
  // If the tab stopped being dirty (saved elsewhere) the dialog self-closes
  // rather than lingering on a discard prompt with nothing left to discard.
  const present = tab !== undefined && tab.dirty

  const path = tab?.path ?? ""

  function handleConfirm() {
    if (!editorCloseTabTarget) return
    editorCloseTab(editorCloseTabTarget.root, editorCloseTabTarget.tabId)
    closeEditorCloseTab()
  }

  return (
    <Modal
      kind="confirm"
      open={editorCloseTabTarget !== null}
      vanished={!present}
      onClose={closeEditorCloseTab}
      title="Discard unsaved changes?"
      cancelLabel="Keep editing"
      confirm={{ label: "Discard & close", tone: "destructive", onSelect: handleConfirm }}
    >
      <p className="text-sm text-destructive">
        Your edits to <InlineCode>{path}</InlineCode> haven&rsquo;t
        been saved. They will be lost.
      </p>
    </Modal>
  )
}
