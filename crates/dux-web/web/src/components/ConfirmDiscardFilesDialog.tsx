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
import {
  changedFileCount,
  countWords,
  discardLeftOutReason,
  fileStatusMeta,
} from "@/lib/changedFiles"
import { formatRegularCount } from "@/lib/formatRegularCount"
import type { ChangedFileView } from "@/lib/types"

const NO_TARGETS: ChangedFileView[] = []
const NO_REASONS: string[] = []
const EMPTY_SUMMARY = {
  untracked: 0,
  tracked: 0,
  repositories: 0,
  folders: 0,
  nestedInside: 0,
}

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
  const summary = useMemo(() => {
    if (paths.length === 0) {
      return { ...EMPTY_SUMMARY, targets: NO_TARGETS, leftOut: NO_REASONS }
    }
    const checked = new Set(paths)
    const selected = unstaged.filter((f) => checked.has(f.path))
    // Rows a delete would not act on are left out of the count and of the
    // request, and named with their reason, rather than sent to be refused.
    const leftOut: string[] = []
    const targets: ChangedFileView[] = []
    for (const f of selected) {
      const reason = discardLeftOutReason(f)
      if (reason === null) targets.push(f)
      else leftOut.push(reason)
    }
    // A folded folder counts the files inside it; a repository of its own is
    // counted apart, because it goes with its history.
    let untracked = 0
    let tracked = 0
    let repositories = 0
    let folders = 0
    let nestedInside = 0
    for (const f of targets) {
      if (f.kind === "nested_repository") {
        repositories += 1
      } else if (fileStatusMeta(f.status).kind === "untracked") {
        untracked += changedFileCount(f)
        if (f.kind === "directory") {
          folders += 1
          nestedInside += f.nested_repositories ?? 0
        }
      } else {
        tracked += changedFileCount(f)
      }
    }
    return { targets, leftOut, untracked, tracked, repositories, folders, nestedInside }
  }, [paths, unstaged])
  const { targets, leftOut, untracked, tracked, repositories, folders, nestedInside } =
    summary
  // Closes itself once every checked path has left the unstaged list, rather
  // than lingering with copy about files that are no longer there.
  const isOpen = useVanishedTargetGuard(
    open,
    targets.length + leftOut.length > 0,
    onCancel,
  )

  const deleted = `${countWords(untracked, "untracked file", "untracked files")} will be permanently DELETED from disk`
  const restored = `${countWords(tracked, "tracked file", "tracked files")} will be restored to ${
    tracked === 1 ? "its" : "their"
  } last committed state`
  const outcomes = [
    untracked > 0 ? deleted : null,
    tracked > 0 ? restored : null,
  ].filter((part): part is string => part !== null)
  const sentences: string[] = []
  if (outcomes.length > 0) sentences.push(`${outcomes.join(", and ")}.`)
  if (repositories > 0) {
    sentences.push(
      `${countWords(repositories, "repository of its own", "repositories of their own")} will be deleted whole, including ${
        repositories === 1 ? "its" : "their"
      } history and any commits not pushed anywhere else.`,
    )
  }
  if (folders > 0) {
    sentences.push("Files the repository ignores inside the folders are kept.")
    if (nestedInside > 0) {
      sentences.push(
        `The ${countWords(nestedInside, "nested repository", "nested repositories")} inside them ${
          nestedInside === 1 ? "is" : "are"
        } kept.`,
      )
    }
  }
  if (leftOut.length > 0) {
    sentences.push(
      `${formatRegularCount(leftOut.length, "selected row")} ${
        leftOut.length === 1 ? "is" : "are"
      } left out: ${leftOut.join("; ")}.`,
    )
  }
  sentences.push("This action cannot be undone.")
  const body = sentences.join(" ")

  return (
    <Dialog open={isOpen} onOpenChange={(next) => !next && onCancel()}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>
            Discard changes to{" "}
            {formatRegularCount(
              targets.reduce((sum, f) => sum + changedFileCount(f), 0),
              "file",
            )}
            ?
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
            // Nothing left to delete once every selected row is left out.
            disabled={targets.length === 0}
            onClick={() => onConfirm(targets.map((f) => f.path))}
          >
            Discard
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
