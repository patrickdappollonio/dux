import { Modal } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"

export interface ReopenEncodingTarget {
  tabId: string
  path: string
  // The WHATWG name to reopen in, and the label the dialog says it by.
  encoding: string
  label: string
}

interface ConfirmReopenEncodingDialogProps {
  target: ReopenEncodingTarget | null
  // Whether there is still something to lose: the tab is still open and still
  // dirty. Recomputed live, so a save or a revert elsewhere closes the dialog.
  present: boolean
  onClose: () => void
  onConfirm: () => void
}

// Reopening reads the file again in another encoding and replaces the buffer
// with it, so with unsaved edits it is the same destructive act as the disk
// banner's reload, and it follows the same template. A clean buffer reopens
// without asking: there is nothing of the person's in it to lose.
export function ConfirmReopenEncodingDialog({
  target,
  present,
  onClose,
  onConfirm,
}: ConfirmReopenEncodingDialogProps) {
  const path = target?.path ?? ""
  const label = target?.label ?? ""

  return (
    <Modal
      kind="confirm"
      open={target !== null}
      vanished={!present}
      onClose={onClose}
      title="Discard your edits and reopen?"
      cancelLabel="Keep my edits"
      confirm={{ label: "Discard & reopen", tone: "destructive", onSelect: onConfirm }}
    >
      <p className="text-sm text-destructive">
        Reopening <InlineCode>{path}</InlineCode> as {label} reads it again
        from disk and replaces everything you have typed here. Your edits are
        not saved anywhere and cannot be recovered.
      </p>
    </Modal>
  )
}
