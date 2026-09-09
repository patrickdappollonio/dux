import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"

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
  const isOpen = useVanishedTargetGuard(target !== null, present, onClose)
  const path = target?.path ?? ""

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Discard your edits and reload?</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          <span className="font-mono break-all">{path}</span> changed on disk.
          Reloading replaces everything you have typed here with the file as it
          is now. Your edits are not saved anywhere and cannot be recovered.
        </p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onClose}>
            Keep my edits
          </Button>
          <Button variant="destructive" onClick={onConfirm}>
            Discard & reload
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
