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
  onClose: () => void
  onConfirm: () => void
}

// Destructive confirm dialog for deleting a file or folder, mirroring
// ConfirmDiscardFileDialog's structure exactly (DialogContent destructive,
// text-destructive body, h-2 misclick spacer, Cancel autoFocus + destructive
// confirm). Deletion is permanent: there is no trash on the server.
export function DeleteEntryDialog({
  target,
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
        <p className="text-sm text-destructive">
          {isDir ? (
            <>
              <span className="font-mono">{path}</span> and everything inside
              it will be permanently deleted from disk (recursive). This
              cannot be undone.
            </>
          ) : (
            <>
              <span className="font-mono">{path}</span> will be permanently
              deleted from disk. This cannot be undone.
            </>
          )}{" "}
          There is no trash on the server.
        </p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onClose}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={onConfirm}>
            Delete
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
