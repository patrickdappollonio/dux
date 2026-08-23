import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

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

// Destructive confirm dialog for deleting a file or folder, mirroring
// ConfirmDiscardFileDialog's structure exactly (DialogContent destructive,
// text-destructive body, h-2 misclick spacer, Cancel autoFocus + destructive
// confirm). Deletion is permanent: there is no trash on the server.
export function DeleteEntryDialog({
  target,
  blockedBySave = false,
  onClose,
  onConfirm,
}: DeleteEntryDialogProps) {
  const path = target?.path ?? ""
  const isDir = target?.isDir ?? false

  return (
    <Dialog
      open={target !== null}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Delete {path}?</DialogTitle>
        </DialogHeader>
        {blockedBySave ? (
          <p className="text-sm text-destructive">
            <span className="font-mono break-all">{path}</span> is currently being
            saved. Wait for the save to finish before deleting it.
          </p>
        ) : (
          <p className="text-sm text-destructive">
            {isDir ? (
              <>
                <span className="font-mono break-all">{path}</span> and everything
                inside it will be permanently deleted from disk (recursive).
                This cannot be undone.
              </>
            ) : (
              <>
                <span className="font-mono break-all">{path}</span> will be permanently
                deleted from disk. This cannot be undone.
              </>
            )}{" "}
            There is no trash on the server.
          </p>
        )}
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={blockedBySave}
            onClick={onConfirm}
          >
            Delete
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
