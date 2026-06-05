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
  checkoutDefaultBranch,
  closeCheckoutDefaultBranch,
  useDux,
} from "@/lib/store"

// Confirms switching a project's SOURCE checkout back to its default branch.
// The server inspects the repo and decides the target branch (it may already be
// on the default, or be unable to determine one), so the copy describes the
// action generically. Mirrors the TUI's checkout flow, which moves HEAD in the
// shared source checkout — hence the confirmation here.
export function CheckoutDefaultBranchDialog() {
  const { checkoutDefaultBranchTarget, viewModel } = useDux()

  const isOpen = checkoutDefaultBranchTarget !== null
  const project = viewModel?.projects.find(
    (p) => p.id === checkoutDefaultBranchTarget,
  )
  const name = project?.name ?? "this project"

  function handleConfirm() {
    if (!checkoutDefaultBranchTarget) return
    checkoutDefaultBranch(checkoutDefaultBranchTarget)
    closeCheckoutDefaultBranch()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeCheckoutDefaultBranch()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Checkout default branch?</DialogTitle>
          <DialogDescription>
            This switches the source checkout for &ldquo;{name}&rdquo; back to its
            default branch, moving HEAD in the shared repository. New agents
            branch from whatever the source checkout is on, so this affects every
            new worktree.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={closeCheckoutDefaultBranch}>
            Cancel
          </Button>
          <Button onClick={handleConfirm}>Checkout default branch</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
