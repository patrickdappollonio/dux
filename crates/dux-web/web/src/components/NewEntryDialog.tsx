import { useState } from "react"

import { ErrorLine } from "@/components/ErrorLine"
import { Modal, useHeldWhileClosed, useModalLifetime } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { Input } from "@/components/ui/input"
import { validateEntryName } from "@/lib/fileTreeOps"
import { renderProse } from "@/lib/prose"

export interface NewEntryTarget {
  kind: "file" | "folder"
  dir: string
}

interface NewEntryDialogProps {
  target: NewEntryTarget | null
  /** The folder to create in has left the tree: the dialog closes itself. */
  vanished?: boolean
  /** Why this page may not write here now (another device drives the row),
   * or null: the dialog's own action is disabled with it. */
  writeBlocked?: string | null
  onClose: () => void
  // Resolves (or rejects, e.g. server 400) once the create request settles.
  onSubmit: (name: string) => Promise<void>
}

// The one New File and New Folder dialog, driven by `target`, whose `kind`
// switches the title and placeholder. Its body is keyed by the open, so its
// local `name` state resets on every one.
export function NewEntryDialog({
  target,
  vanished,
  writeBlocked,
  onClose,
  onSubmit,
}: NewEntryDialogProps) {
  const life = useModalLifetime(target !== null)
  return life.mounted ? (
    <NewEntryDialogBody
      key={life.epoch}
      target={target}
      vanished={vanished}
      writeBlocked={writeBlocked}
      onClose={onClose}
      onClosed={life.onClosed}
      onSubmit={onSubmit}
    />
  ) : null
}

function NewEntryDialogBody({
  target,
  vanished = false,
  writeBlocked = null,
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
  // A refusal that follows typing: silent as a live region, and read with the
  // field it is about.
  const refused = name.length > 0 && !validation.ok

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
      vanished={target !== null && vanished}
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
      writeBlocked={writeBlocked}
    >
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder={place.kind === "file" ? "example.ts" : "components"}
        aria-invalid={refused}
        aria-describedby={refused ? "new-entry-error" : undefined}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault()
            submit()
          }
        }}
      />
      {refused && (
        <ErrorLine announce="silent" id="new-entry-error">
          {renderProse(validation.error)}
        </ErrorLine>
      )}
    </Modal>
  )
}
