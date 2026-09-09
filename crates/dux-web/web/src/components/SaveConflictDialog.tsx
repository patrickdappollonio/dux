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

// What a refused save looks like to the user: the file is not what this buffer
// was read from, so somebody else's work is on disk. Three honest answers, all
// offered: keep this version and overwrite theirs, take the disk version through
// its own destructive confirm, or do nothing. Cancel takes focus, doing nothing
// being the only choice that loses no work.
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
