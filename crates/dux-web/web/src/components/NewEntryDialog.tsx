import { useState } from "react"
import { Loader2 } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
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

// Unified New File / New Folder dialog, replacing the old inline "New file"
// prompt. Driven by `target`; title and placeholder switch on `target.kind`.
// The body is mounted only while `target` is non-null, so its local `name`
// state resets on every open with no set-state-in-effect (matches
// AddProjectDialog's pattern).
export function NewEntryDialog({
  target,
  onClose,
  onSubmit,
}: NewEntryDialogProps) {
  return (
    <Dialog
      open={target !== null}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent showCloseButton={false} className="sm:max-w-md">
        {target && (
          <NewEntryDialogBody
            target={target}
            onClose={onClose}
            onSubmit={onSubmit}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

function NewEntryDialogBody({
  target,
  onClose,
  onSubmit,
}: {
  target: NewEntryTarget
  onClose: () => void
  onSubmit: (name: string) => Promise<void>
}) {
  const [name, setName] = useState("")
  const [submitting, setSubmitting] = useState(false)

  const validation = validateEntryName(name)
  const canSubmit = name.trim().length > 0 && validation.ok && !submitting

  function submit(): void {
    if (!canSubmit) return
    setSubmitting(true)
    onSubmit(name.trim()).finally(() => setSubmitting(false))
  }

  const dirLabel = target.dir === "" ? "/" : target.dir
  const title = target.kind === "file" ? "New file" : "New folder"

  return (
    <>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>
          in <span className="font-mono">{dirLabel}</span>
        </DialogDescription>
      </DialogHeader>
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder={target.kind === "file" ? "example.ts" : "components"}
        autoFocus
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
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <Button disabled={!canSubmit} aria-busy={submitting} onClick={submit}>
          {submitting ? <Loader2 className="motion-safe:animate-spin" /> : null}
          Create
        </Button>
      </DialogFooter>
    </>
  )
}
