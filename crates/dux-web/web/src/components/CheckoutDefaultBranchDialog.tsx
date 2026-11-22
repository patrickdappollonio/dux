import { Modal } from "@/components/modal"
import { checkoutDefaultBranchProse } from "@/lib/checkoutDefaultBranch"
import { renderProse } from "@/lib/prose"
import {
  checkoutDefaultBranch,
  closeCheckoutDefaultBranch,
  useDux,
} from "@/lib/store"

// Confirms switching a project's SOURCE checkout back to its default branch, which
// moves HEAD in a shared checkout. The server decides the target branch, so the
// copy describes the action generically.
export function CheckoutDefaultBranchDialog() {
  const { checkoutDefaultBranchTarget, spine } = useDux()

  const project = spine?.projects.find(
    (p) => p.id === checkoutDefaultBranchTarget,
  )
  const name = project?.name ?? "this project"

  function handleConfirm() {
    if (!checkoutDefaultBranchTarget) return
    checkoutDefaultBranch(checkoutDefaultBranchTarget)
    closeCheckoutDefaultBranch()
  }

  return (
    <Modal
      kind="confirm"
      open={checkoutDefaultBranchTarget !== null}
      // Checking out a branch of a project that no longer exists is moot.
      vanished={project === undefined}
      onClose={closeCheckoutDefaultBranch}
      title="Check out default branch?"
      description={renderProse(
        checkoutDefaultBranchProse(name, project?.leading_branch ?? null),
      )}
      cancelLabel="Cancel"
      // The terminal UI's confirm button reads the same.
      confirm={{ label: "Check out default branch", tone: "primary", onSelect: handleConfirm }}
    />
  )
}
