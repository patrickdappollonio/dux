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
  // Whether there is still something to confirm: the tab is still dirty AND
  // the file on disk is still different. The caller recomputes it from the
  // live buffer, so the dialog self-closes if the user saves, reverts, or
  // reloads from another surface while it is open. See the guard's own doc
  // for why a target-keyed dialog needs this.
  present: boolean
  onClose: () => void
  onConfirm: () => void
}

// Destructive confirm before replacing an edited buffer with what is on disk.
//
// The destructive act is discarding the USER'S text, which is why this follows
// ConfirmCloseEditorTabDialog's template rather than being a plain yes/no:
// destructive DialogContent, the warning in destructive colour, a misclick
// spacer above the footer, Cancel taking focus, and the confirm styled as the
// dangerous one. A clean buffer never reaches here: with nothing to lose the
// editor reloads in place with no prompt at all.
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
