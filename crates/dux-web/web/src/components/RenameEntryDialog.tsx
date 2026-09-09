import { useState } from "react"
import { Loader2 } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
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
  return (
    <Dialog
      open={target !== null}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent showCloseButton={false} className="sm:max-w-md">
        {target && (
          <RenameEntryDialogBody
            target={target}
            isDirty={isDirty}
            onClose={onClose}
            onSubmit={onSubmit}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

function RenameEntryDialogBody({
  target,
  isDirty,
  onClose,
  onSubmit,
}: {
  target: RenameEntryTarget
  isDirty: boolean
  onClose: () => void
  onSubmit: (newName: string) => Promise<void>
}) {
  const [name, setName] = useState(() => finalSegment(target.path))
  const [submitting, setSubmitting] = useState(false)

  const validation = validateEntryName(name)
  const canSubmit =
    !isDirty && name.trim().length > 0 && validation.ok && !submitting

  function submit(): void {
    if (!canSubmit) return
    setSubmitting(true)
    onSubmit(name.trim()).finally(() => setSubmitting(false))
  }

  return (
    <>
      <DialogHeader>
        <DialogTitle>Rename {finalSegment(target.path)}</DialogTitle>
      </DialogHeader>
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        autoFocus
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
        <p className="text-sm text-destructive">
          Save or discard changes in this file before renaming.
        </p>
      ) : (
        name.length > 0 &&
        !validation.ok && (
          <p className="text-sm text-destructive">{validation.error}</p>
        )
      )}
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <Button disabled={!canSubmit} aria-busy={submitting} onClick={submit}>
          {submitting ? <Loader2 className="motion-safe:animate-spin" /> : null}
          Rename
        </Button>
      </DialogFooter>
    </>
  )
}
