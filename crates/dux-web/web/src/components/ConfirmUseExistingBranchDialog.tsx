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

// Consent before a new agent attaches to an existing branch's history. The
// server refuses an unconfirmed create whose name matches a branch and returns
// a confirmable conflict; the store opens this dialog with the branch name and
// location. Confirm re-creates with `use_existing_branch: true`, Cancel
// abandons the create. Cancel is the default focus.
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
          <DialogTitle>
            Attach to existing branch “<span className="break-all">{name}</span>”?
          </DialogTitle>
          <DialogDescription>
            A branch named “<span className="break-all">{name}</span>” already exists {where}. Creating this agent
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
