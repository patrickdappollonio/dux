import { Modal } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"

export interface DeleteEntryTarget {
  path: string
  isDir: boolean
}

interface DeleteEntryDialogProps {
  target: DeleteEntryTarget | null
  // True when a save for `target.path` is currently in flight (the caller
  // computes this from its `savingPaths` set). When true, Delete is disabled
  // and a blocking note replaces the usual warning copy, mirroring
  // RenameEntryDialog's `isDirty` gate: deleting a path whose write hasn't
  // resolved yet would let that in-flight write silently recreate the file
  // right after the delete lands.
  blockedBySave?: boolean
  onClose: () => void
  onConfirm: () => void
}

// Destructive confirm for deleting a file or folder. Deletion is permanent:
// there is no trash on the server.
export function DeleteEntryDialog({
  target,
  blockedBySave = false,
  onClose,
  onConfirm,
}: DeleteEntryDialogProps) {
  const path = target?.path ?? ""
  const isDir = target?.isDir ?? false

  return (
    <Modal
      kind="confirm"
      open={target !== null}
      onClose={onClose}
      title={
        <>
          Delete <InlineCode>{path}</InlineCode>?
        </>
      }
      cancelLabel="Cancel"
      confirm={{
        label: "Delete",
        tone: "destructive",
        disabledReason: blockedBySave
          ? "Wait for the save to finish before deleting it."
          : null,
        onSelect: onConfirm,
      }}
    >
      {blockedBySave ? (
        <p className="text-sm text-destructive">
          <InlineCode>{path}</InlineCode> is currently being
          saved. Wait for the save to finish before deleting it.
        </p>
      ) : (
        <p className="text-sm text-destructive">
          {isDir ? (
            <>
              <InlineCode>{path}</InlineCode> and everything
              inside it will be permanently deleted from disk (recursive).
              This cannot be undone.
            </>
          ) : (
            <>
              <InlineCode>{path}</InlineCode> will be permanently
              deleted from disk. This cannot be undone.
            </>
          )}{" "}
          There is no trash on the server.
        </p>
      )}
    </Modal>
  )
}
