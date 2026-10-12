import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"

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
  const isOpen = useVanishedTargetGuard(target !== null, present, onClose)
  const path = target?.path ?? ""
  const label = target?.label ?? ""

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Discard your edits and reopen?</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          Reopening <InlineCode>{path}</InlineCode> as {label} reads it again
          from disk and replaces everything you have typed here. Your edits are
          not saved anywhere and cannot be recovered.
        </p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onClose}>
            Keep my edits
          </Button>
          <Button variant="destructive" onClick={onConfirm}>
            Discard & reopen
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
