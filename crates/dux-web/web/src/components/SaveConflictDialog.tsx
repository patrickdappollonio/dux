import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

export interface SaveConflictTarget {
  tabId: string
  path: string
  // The text the refused save was carrying, kept so "Overwrite" can re-send
  // exactly what the user asked to save rather than re-reading the draft,
  // which may have moved on.
  body: string
  // The file is gone rather than merely different: there is nothing to reload,
  // so the offer is to write it back or to cancel.
  deleted: boolean
}

interface SaveConflictDialogProps {
  target: SaveConflictTarget | null
  onOverwrite: () => void
  onReload: () => void
  onClose: () => void
}

// What a 409 from the save route looks like to the user.
//
// The server refused because the file is not what this buffer was read from,
// which means somebody else's work is on disk. There are exactly three honest
// answers and this offers all three: keep the user's version (overwrite, which
// destroys the other edit), take the disk version (reload, which destroys the
// user's text and therefore goes through its own destructive confirm), or do
// nothing. Cancel takes focus, because doing nothing is the only choice that
// loses no work: the draft survives a cancelled save untouched.
//
// Deliberately not a toast: a toast retires itself, and every option here
// destroys something.
export function SaveConflictDialog({
  target,
  onOverwrite,
  onReload,
  onClose,
}: SaveConflictDialogProps) {
  const path = target?.path ?? ""
  const deleted = target?.deleted ?? false

  return (
    <Dialog
      open={target !== null}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>
            {deleted ? "That file is gone" : "The file changed on disk"}
          </DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          {deleted ? (
            <>
              <span className="font-mono break-all">{path}</span> was deleted
              after you opened it, so nothing was saved. You can write your
              version back as a new file at the same path, or cancel and keep
              the text here.
            </>
          ) : (
            <>
              <span className="font-mono break-all">{path}</span> changed after
              you opened it, so nothing was saved. Saving anyway replaces
              whatever is on disk now; reloading replaces what you typed.
            </>
          )}
        </p>
        {/* Misclick-safe spacing: every button below destroys something. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onClose}>
            Cancel
          </Button>
          {!deleted && (
            <Button variant="outline" onClick={onReload}>
              Reload from disk
            </Button>
          )}
          <Button variant="destructive" onClick={onOverwrite}>
            {deleted ? "Write it back" : "Overwrite"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
