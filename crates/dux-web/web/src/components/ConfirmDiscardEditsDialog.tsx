import { Modal } from "@/components/modal/Modal"
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
    <Modal
      kind="confirm"
      open={open}
      onClose={onKeep}
      title="Discard unsaved changes?"
      description={
        <>
          {holder} {unsavedChangesLabel(changes)}. Discarding them leaves{" "}
          {renderProse(leaves)} as it was.
        </>
      }
      cancelLabel="Keep editing"
      confirm={{ label: "Discard changes", tone: "destructive", onSelect: onDiscard }}
      finalFocus={finalFocus}
    />
  )
}
