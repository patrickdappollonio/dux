import { useState } from "react"

import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { Input } from "@/components/ui/input"
import { validateEntryName } from "@/lib/fileTreeOps"

export interface NewEntryTarget {
  kind: "file" | "folder"
  dir: string
}

interface NewEntryDialogProps {
  target: NewEntryTarget | null
  onClose: () => void
  // Resolves (or rejects, e.g. server 400) once the create request settles.
  onSubmit: (name: string) => Promise<void>
}

// The one New File and New Folder dialog, driven by `target`, whose `kind`
// switches the title and placeholder. Its body is keyed by the open, so its
// local `name` state resets on every one.
export function NewEntryDialog({
  target,
  onClose,
  onSubmit,
}: NewEntryDialogProps) {
  const life = useModalLifetime(target !== null)
  return life.mounted ? (
    <NewEntryDialogBody
      key={life.epoch}
      target={target}
      onClose={onClose}
      onClosed={life.onClosed}
      onSubmit={onSubmit}
    />
  ) : null
}

function NewEntryDialogBody({
  target,
  onClose,
  onClosed,
  onSubmit,
}: NewEntryDialogProps & { onClosed: () => void }) {
  // Where this open creates; the last one is kept while the close animates.
  const [last, setLast] = useState(target)
  if (target !== null && target !== last) setLast(target)
  const place = target ?? last ?? { kind: "file" as const, dir: "" }
  const [name, setName] = useState("")
  const [submittingNow, setSubmitting] = useState(false)
  // A create that finishes as the dialog closes keeps its spinner while the
  // close animates.
  const submitting = useHeldWhileClosed(target !== null, submittingNow)

  const validation = validateEntryName(name)
  const canSubmit = name.trim().length > 0 && validation.ok && !submitting

  function submit(): void {
    if (!canSubmit) return
    setSubmitting(true)
    onSubmit(name.trim()).finally(() => setSubmitting(false))
  }

  const dirLabel = place.dir === "" ? "/" : place.dir

  return (
    <Modal
      kind="form"
      open={target !== null}
      onClose={() => onClose()}
      onClosed={onClosed}
      size="md"
      title={place.kind === "file" ? "New file" : "New folder"}
      description={
        <>
          in <InlineCode>{dirLabel}</InlineCode>
        </>
      }
      submit={{
        label: "Create",
        tone: "primary",
        onSelect: submit,
        busy: submitting,
        disabledReason: validation.ok ? null : validation.error,
      }}
    >
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder={place.kind === "file" ? "example.ts" : "components"}
        aria-invalid={name.length > 0 && !validation.ok}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault()
            submit()
          }
        }}
      />
      {name.length > 0 && !validation.ok && (
        <p className="text-sm text-destructive">{validation.error}</p>
      )}
    </Modal>
  )
}
