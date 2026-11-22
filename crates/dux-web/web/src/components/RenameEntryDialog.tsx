import { useState } from "react"

import { Modal, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { Input } from "@/components/ui/input"
import { validateEntryName } from "@/lib/fileTreeOps"

export interface RenameEntryTarget {
  path: string
  isDir: boolean
}

interface RenameEntryDialogProps {
  target: RenameEntryTarget | null
  // True when `target`, or a descendant of it, has unsaved changes. It disables
  // Confirm: renaming a dirty open file reloads it from disk and drops the draft.
  isDirty: boolean
  onClose: () => void
  onSubmit: (newName: string) => Promise<void>
}

// Final path segment: "a/b/old.ts" -> "old.ts"; "old.ts" -> "old.ts".
function finalSegment(path: string): string {
  const idx = path.lastIndexOf("/")
  return idx === -1 ? path : path.slice(idx + 1)
}

// Renaming an open file's tab retargets its `path` in place. Monaco's model is
// keyed by the URI, so the new path loses undo history and view state; that is
// accepted for a CLEAN tab, and the `isDirty` gate refuses a dirty one outright.
export function RenameEntryDialog({
  target,
  isDirty,
  onClose,
  onSubmit,
}: RenameEntryDialogProps) {
  const life = useModalLifetime(target !== null)
  // Keyed by the open, so the name starts from the entry's own each time.
  return life.mounted ? (
    <RenameEntryDialogBody
      key={life.epoch}
      target={target}
      isDirty={isDirty}
      onClose={onClose}
      onClosed={life.onClosed}
      onSubmit={onSubmit}
    />
  ) : null
}

function RenameEntryDialogBody({
  target,
  isDirty,
  onClose,
  onClosed,
  onSubmit,
}: RenameEntryDialogProps & { onClosed: () => void }) {
  // The entry this open is about; the last one is kept while the close animates.
  const [last, setLast] = useState(target)
  if (target !== null && target !== last) setLast(target)
  const path = (target ?? last)?.path ?? ""
  const [name, setName] = useState(() => finalSegment(path))
  const [submitting, setSubmitting] = useState(false)

  const validation = validateEntryName(name)
  const canSubmit =
    !isDirty && name.trim().length > 0 && validation.ok && !submitting
  const dirtyRefusal = "Save or discard changes in this file before renaming."

  function submit(): void {
    if (!canSubmit) return
    setSubmitting(true)
    onSubmit(name.trim()).finally(() => setSubmitting(false))
  }

  return (
    <Modal
      kind="form"
      open={target !== null}
      onClose={() => onClose()}
      onClosed={onClosed}
      size="md"
      title={
        <>
          Rename <InlineCode>{finalSegment(path)}</InlineCode>
        </>
      }
      submit={{
        label: "Rename",
        tone: "primary",
        onSelect: submit,
        busy: submitting,
        disabledReason: isDirty ? dirtyRefusal : validation.ok ? null : validation.error,
      }}
    >
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        onFocus={(e) => e.currentTarget.select()}
        disabled={isDirty}
        aria-invalid={name.length > 0 && !validation.ok}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault()
            submit()
          }
        }}
      />
      {isDirty ? (
        <p className="text-sm text-destructive">{dirtyRefusal}</p>
      ) : (
        name.length > 0 &&
        !validation.ok && <p className="text-sm text-destructive">{validation.error}</p>
      )}
    </Modal>
  )
}
