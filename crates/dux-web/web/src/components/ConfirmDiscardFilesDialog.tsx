import { useMemo } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { fileStatusMeta } from "@/lib/changedFiles"
import { formatRegularCount } from "@/lib/formatRegularCount"
import type { ChangedFileView } from "@/lib/types"

const NO_TARGETS: ChangedFileView[] = []

interface Props {
  open: boolean
  // The checked paths, which may name a file that has already left the list.
  paths: string[]
  // The live unstaged files, which decide both the copy and what is discarded.
  unstaged: ChangedFileView[]
  onCancel: () => void
  onConfirm: (paths: string[]) => void
}

// Confirmation before discarding a whole checked selection. The two outcomes
// are split per file from the LIVE list rather than from anything the caller
// supplied: an untracked file is permanently deleted, a tracked one is restored
// from its last committed state. The dialog acts on that same intersection, so
// a file that left the list between the click and the confirm is not discarded.
export function ConfirmDiscardFilesDialog({
  open,
  paths,
  unstaged,
  onCancel,
  onConfirm,
}: Props) {
  // The dialog stays mounted beside a list that can hold tens of thousands of
  // files, so the intersection is recomputed only when the selection or the
  // list moves, never on a render that changed neither. An empty selection,
  // the usual state, never walks the list at all.
  const { targets, untracked } = useMemo(() => {
    if (paths.length === 0) return { targets: NO_TARGETS, untracked: 0 }
    const checked = new Set(paths)
    const targets = unstaged.filter((f) => checked.has(f.path))
    const untracked = targets.filter(
      (f) => fileStatusMeta(f.status).kind === "untracked",
    ).length
    return { targets, untracked }
  }, [paths, unstaged])
  // Closes itself once every checked path has left the unstaged list, rather
  // than lingering with copy about files that are no longer there.
  const isOpen = useVanishedTargetGuard(open, targets.length > 0, onCancel)

  const tracked = targets.length - untracked
  const deleted = `${formatRegularCount(untracked, "untracked file")} will be permanently DELETED from disk`
  const restored = `${formatRegularCount(tracked, "tracked file")} will be restored to ${
    tracked === 1 ? "its" : "their"
  } last committed state`
  const body =
    untracked > 0 && tracked > 0
      ? `${deleted}, and ${restored}. This action cannot be undone.`
      : untracked > 0
        ? `${deleted}. This action cannot be undone.`
        : `${restored}. This action cannot be undone.`

  return (
    <Dialog open={isOpen} onOpenChange={(next) => !next && onCancel()}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>
            Discard changes to {formatRegularCount(targets.length, "file")}?
          </DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">{body}</p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            onClick={() => onConfirm(targets.map((f) => f.path))}
          >
            Discard
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
