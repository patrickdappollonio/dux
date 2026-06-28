import { useEffect } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { closeDiscard, discardFile, useDux } from "@/lib/store"

// Confirmation before discarding an unstaged file's changes. The TUI confirms
// every discard because it is destructive and cannot be undone, with Cancel as
// the default focus — the web mirrors that. The body copy distinguishes the two
// outcomes the way the TUI's discard semantics do: a tracked file is restored
// from its last committed state, while an untracked file is permanently DELETED.
export function ConfirmDiscardFileDialog() {
  const { discardTarget, changes } = useDux()

  // If the file leaves the unstaged list while the dialog is open (committed
  // or staged elsewhere, or already discarded), close rather than linger on a
  // stale path with possibly-wrong restore-vs-DELETE copy. Mirrors the
  // vanished-target handling in ConfirmDeleteTerminalDialog; the external-store
  // call is not a React setState, so the set-state-in-effect rule doesn't bite.
  // Trust the changes slice only when it belongs to the discard target's session.
  const stillUnstaged =
    discardTarget !== null &&
    changes.sessionId === discardTarget.sessionId &&
    changes.unstaged.some((f) => f.path === discardTarget.path)
  useEffect(() => {
    if (discardTarget && !stillUnstaged) {
      closeDiscard()
    }
  }, [discardTarget, stillUnstaged])

  const isOpen = discardTarget !== null && stillUnstaged
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
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Discard changes to {path}?</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          {untracked ? (
            <>
              <span className="font-mono">{path}</span> is untracked and will be{" "}
              permanently DELETED from disk. This action cannot be undone.
            </>
          ) : (
            <>
              All changes to <span className="font-mono">{path}</span> will be{" "}
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
