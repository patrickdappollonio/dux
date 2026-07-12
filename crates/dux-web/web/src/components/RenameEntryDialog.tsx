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
  // True when `target` (or, for a folder, a descendant of it) has unsaved
  // changes. Computed by the caller from `hasDirtyUnderPath` so this component
  // stays pure UI. When true, Confirm is disabled and a blocking note is shown
  // instead of the usual validation error: renaming a dirty open file would
  // reload it from disk after the move and silently drop the draft.
  isDirty: boolean
  onClose: () => void
  onSubmit: (newName: string) => Promise<void>
}

// Final path segment: "a/b/old.ts" -> "old.ts"; "old.ts" -> "old.ts".
function finalSegment(path: string): string {
  const idx = path.lastIndexOf("/")
  return idx === -1 ? path : path.slice(idx + 1)
}

// Renaming an open file's tab retargets its `path` in place (see
// `editorTabs.ts` `renameTabPaths`) rather than closing and reopening it. For
// a CLEAN tab that is a deliberate, accepted tradeoff: Monaco's model is keyed
// by the path's URI, so the new path gets a brand-new model with no undo
// history or view state (folding, scroll, cursor). We only reach that retarget
// for a clean tab, though: the `isDirty` gate below refuses to rename a
// dirty one at all, so an in-progress edit is never silently reloaded away.
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
