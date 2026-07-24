import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  closeExistingBranch,
  confirmCreateWithExistingBranch,
  useDux,
} from "@/lib/store"

// Consent before a new agent ATTACHES to an existing branch's history. The
// server refuses an unconfirmed create whose name matches a branch (the "no
// silent attach" tenet) and returns a confirmable 409; the store opens this
// dialog with the branch name + location. Confirm re-creates with
// `use_existing_branch: true`; Cancel abandons the create so the user can pick a
// different name. Mirrors the TUI's ConfirmUseExistingBranch prompt. Cancel is
// the default focus, matching the other confirm dialogs.
export function ConfirmUseExistingBranchDialog() {
  const { existingBranchTarget } = useDux()
  const isOpen = existingBranchTarget !== null
  const name = existingBranchTarget?.name ?? ""
  const where =
    existingBranchTarget?.location === "remote"
      ? "on the remote (origin)"
      : "locally"

  function handleOpenChange(open: boolean) {
    if (!open) closeExistingBranch()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Attach to existing branch “{name}”?</DialogTitle>
          <DialogDescription>
            A branch named “{name}” already exists {where}. Creating this agent
            will attach to that branch and adopt its history, not start a fresh
            branch. Continue, or cancel and pick a different name.
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeExistingBranch}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            onClick={confirmCreateWithExistingBranch}
          >
            Attach to branch
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
