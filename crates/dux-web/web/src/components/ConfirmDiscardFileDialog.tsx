import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { closeDiscard, discardFile, useDux } from "@/lib/store"

// Confirmation before discarding an unstaged file's changes, which cannot be
// undone. The copy distinguishes the outcomes: a tracked file is restored from its
// last committed state, an untracked file is permanently DELETED.
export function ConfirmDiscardFileDialog() {
  const { discardTarget, changes } = useDux()

  // Trust the changes slice only when it belongs to the discard target's session.
  const stillUnstaged =
    discardTarget !== null &&
    changes.sessionId === discardTarget.sessionId &&
    changes.unstaged.some((f) => f.path === discardTarget.path)
  // Closes when the file leaves the unstaged list, rather than lingering on a
  // stale path whose restore-versus-DELETE copy may now be wrong.
  const isOpen = useVanishedTargetGuard(
    discardTarget !== null,
    stillUnstaged,
    closeDiscard,
  )
  const path = discardTarget?.path ?? ""
  const untracked = discardTarget?.untracked ?? false

  function handleConfirm() {
    if (!discardTarget) return
    discardFile(discardTarget.sessionId, discardTarget.path)
    closeDiscard()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeDiscard()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Discard changes to {path}?</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          {untracked ? (
            <>
              <span className="font-mono break-all">{path}</span> is untracked and will be{" "}
              permanently DELETED from disk. This action cannot be undone.
            </>
          ) : (
            <>
              All changes to <span className="font-mono break-all">{path}</span> will be{" "}
              restored to its last committed state. This action cannot be undone.
            </>
          )}
        </p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          {/* Cancel is the default focus, matching the TUI (Cancel highlighted).
              shadcn/radix buttons already activate on Space/Enter natively. */}
          <Button variant="outline" autoFocus onClick={closeDiscard}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Discard
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
