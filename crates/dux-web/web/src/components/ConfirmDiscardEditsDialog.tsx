import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { renderProse, type Prose } from "@/lib/prose"
import { unsavedChangesLabel } from "@/lib/unsavedChanges"

// Closing a form with unsaved changes asks first: they exist only in that
// form. `holder` opens the sentence, as in "The macros have", and `leaves`
// names what discarding leaves as it was.
export function ConfirmDiscardEditsDialog({
  open,
  holder,
  changes,
  leaves,
  onKeep,
  finalFocus,
  onDiscard,
}: {
  open: boolean
  holder: string
  changes: number
  leaves: Prose
  onKeep: () => void
  finalFocus?: () => HTMLElement | boolean
  onDiscard: () => void
}) {
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onKeep()
      }}
    >
      <DialogContent showCloseButton={false} destructive finalFocus={finalFocus}>
        <DialogHeader>
          <DialogTitle>Discard unsaved changes?</DialogTitle>
          <DialogDescription>
            {holder} {unsavedChangesLabel(changes)}. Discarding them leaves{" "}
            {renderProse(leaves)} as it was.
          </DialogDescription>
        </DialogHeader>
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onKeep}>
            Keep editing
          </Button>
          <Button variant="destructive" onClick={onDiscard}>
            Discard changes
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
