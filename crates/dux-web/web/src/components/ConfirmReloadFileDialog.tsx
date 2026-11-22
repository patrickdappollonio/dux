import { Modal } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"

export interface ReloadFileTarget {
  tabId: string
  path: string
}

interface ConfirmReloadFileDialogProps {
  target: ReloadFileTarget | null
  // Whether there is still something to confirm: still dirty AND still stale.
  // Recomputed live, so the dialog self-closes if another surface resolves it.
  present: boolean
  onClose: () => void
  onConfirm: () => void
}

// Destructive confirm before replacing an edited buffer with what is on disk: the
// destructive act is discarding the user's text, so this follows the destructive
// template rather than a plain yes/no. A clean buffer reloads in place, unprompted.
export function ConfirmReloadFileDialog({
  target,
  present,
  onClose,
  onConfirm,
}: ConfirmReloadFileDialogProps) {
  const path = target?.path ?? ""

  return (
    <Modal
      kind="confirm"
      open={target !== null}
      vanished={!present}
      onClose={onClose}
      title="Discard your edits and reload?"
      cancelLabel="Keep my edits"
      confirm={{ label: "Discard & reload", tone: "destructive", onSelect: onConfirm }}
    >
      <p className="text-sm text-destructive">
        <InlineCode>{path}</InlineCode> changed on disk.
        Reloading replaces everything you have typed here with the file as it
        is now. Your edits are not saved anywhere and cannot be recovered.
      </p>
    </Modal>
  )
}
