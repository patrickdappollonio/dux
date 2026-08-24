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
import type { ChangedFileView } from "@/lib/types"

interface Props {
  open: boolean
  // The checked paths, which may name a file that has already left the list.
  paths: string[]
  // The live unstaged files, which decide both the copy and what is discarded.
  unstaged: ChangedFileView[]
  onCancel: () => void
  onConfirm: (paths: string[]) => void
}

function count(n: number, noun: string) {
  return `${n} ${noun}${n === 1 ? "" : "s"}`
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
  const checked = new Set(paths)
  const targets = unstaged.filter((f) => checked.has(f.path))
  // Closes itself once every checked path has left the unstaged list, rather
  // than lingering with copy about files that are no longer there.
  const isOpen = useVanishedTargetGuard(open, targets.length > 0, onCancel)

  const untracked = targets.filter(
    (f) => fileStatusMeta(f.status).kind === "untracked",
  ).length
  const tracked = targets.length - untracked
  const deleted = `${count(untracked, "untracked file")} will be permanently DELETED from disk`
  const restored = `${count(tracked, "tracked file")} will be restored to ${
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
            Discard changes to {count(targets.length, "file")}?
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
