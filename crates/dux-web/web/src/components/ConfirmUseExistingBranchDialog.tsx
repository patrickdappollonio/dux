import { Modal } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
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
  const name = existingBranchTarget?.name ?? ""
  const where =
    existingBranchTarget?.location === "remote"
      ? "on the remote (origin)"
      : "locally"

  return (
    <Modal
      kind="confirm"
      open={existingBranchTarget !== null}
      onClose={closeExistingBranch}
      // Raised by the server's refusal, not by a click, so it takes no history
      // entry for the browser's Back.
      historyEntry={false}
      title={
        <>
          Attach to existing branch <InlineCode>{name}</InlineCode>?
        </>
      }
      description={
        <>
          A branch named <InlineCode>{name}</InlineCode> already exists{" "}
          {where}. Creating this agent
          will attach to that branch and adopt its history, not start a fresh
          branch. Continue, or cancel and pick a different name.
        </>
      }
      cancelLabel="Cancel"
      // Primary, not destructive: attaching adopts the branch's history and
      // changes nothing in it.
      confirm={{
        label: "Attach to branch",
        tone: "primary",
        onSelect: confirmCreateWithExistingBranch,
      }}
    />
  )
}
