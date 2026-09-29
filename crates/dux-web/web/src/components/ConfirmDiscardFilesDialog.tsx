import { useEffect, useMemo, useState } from "react"

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
  deletableFileCount,
  fileStatusMeta,
} from "@/lib/changedFiles"
import { formatRegularCount } from "@/lib/formatRegularCount"
import {
  bareRepositoriesKept,
  changedWhileOpen,
  discardConfirmation,
  discardLeftOutReason,
  firstChangedKind,
} from "@/lib/discardOutcome"
import type { DiscardConfirmation } from "@/lib/git"
import { notifyWarning } from "@/lib/notify"
import { joinProse, prose, renderProse, type Prose } from "@/lib/prose"
import type { ChangedFileView } from "@/lib/types"

const NO_TARGETS: ChangedFileView[] = []
const NO_REASONS: Prose[] = []
const NO_BARE: string[] = []
const EMPTY_SUMMARY = {
  bare: NO_BARE,
  filesInBare: 0,
  untracked: 0,
  tracked: 0,
  repositories: 0,
  folders: 0,
  nestedInside: 0,
  worktreesInside: 0,
}

interface Props {
  open: boolean
  // The checked paths, which may name a file that has already left the list.
  paths: string[]
  // The live unstaged files. The rows the dialog opens on are taken from
  // them at that moment; afterwards they only say which rows are still there
  // and whether one changed kind.
  unstaged: ChangedFileView[]
  onCancel: () => void
  // The paths to discard, and what each folder among them was when the
  // dialog opened (its kind and file count), which is what the user confirmed.
  onConfirm: (paths: string[], confirmations: Record<string, DiscardConfirmation>) => void
}

// Confirmation before discarding a whole checked selection. The rows are
// taken from the live list when the dialog OPENS, and the copy and the request
// both come from them: an untracked file is permanently deleted, a tracked one
// is restored from its last committed state. A row that has since left the
// list is dropped, and one that has changed kind (a folder turned repository)
// closes the dialog with a notice, because what the user would be confirming
// is no longer what is there.
export function ConfirmDiscardFilesDialog({
  open,
  paths,
  unstaged,
  onCancel,
  onConfirm,
}: Props) {
  // The rows as they were when the dialog opened, captured on the render
  // that opens it and dropped on the one that closes it.
  const [openedOn, setOpenedOn] = useState<ChangedFileView[] | null>(null)
  if (open && openedOn === null) {
    const checked = new Set(paths)
    setOpenedOn(paths.length === 0 ? NO_TARGETS : unstaged.filter((f) => checked.has(f.path)))
  } else if (!open && openedOn !== null) {
    setOpenedOn(null)
  }
  const rows = openedOn ?? NO_TARGETS
  const changed = useMemo(
    () => (open ? firstChangedKind(rows, unstaged) : null),
    [open, rows, unstaged],
  )
  useEffect(() => {
    if (changed === null) return
    notifyWarning(changedWhileOpen(changed.before, changed.now))
    onCancel()
  }, [changed, onCancel])
  // The dialog stays mounted beside a list that can hold tens of thousands of
  // files, so the intersection is recomputed only when the opened-on rows or
  // the list move, never on a render that changed neither. A closed dialog,
  // the usual state, never walks the list at all.
  const summary = useMemo(() => {
    if (rows.length === 0) {
      return { ...EMPTY_SUMMARY, targets: NO_TARGETS, leftOut: NO_REASONS }
    }
    const present = new Set(unstaged.map((f) => f.path))
    const selected = rows.filter((f) => present.has(f.path))
    // Rows a delete would not act on are left out of the count and of the
    // request, and named with their reason, rather than sent to be refused.
    const leftOut: Prose[] = []
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
    let worktreesInside = 0
    const bare: string[] = []
    let filesInBare = 0
    for (const f of targets) {
      if (f.kind === "nested_repository") {
        repositories += 1
      } else if (fileStatusMeta(f.status).kind === "untracked") {
        // A folder delete keeps the bare repositories inside, whose files
        // the row counts, so only the rest is said to go.
        untracked += deletableFileCount(f)
        if (f.kind === "directory") {
          folders += 1
          nestedInside += f.nested_repositories ?? 0
          worktreesInside += f.linked_worktrees ?? 0
          bare.push(...(f.bare_repositories ?? []))
          filesInBare += f.files_in_bare_repositories ?? 0
        }
      } else {
        tracked += changedFileCount(f)
      }
    }
    return {
      targets,
      leftOut,
      bare,
      filesInBare,
      untracked,
      tracked,
      repositories,
      folders,
      nestedInside,
      worktreesInside,
    }
  }, [rows, unstaged])
  const {
    targets,
    leftOut,
    bare,
    filesInBare,
    untracked,
    tracked,
    repositories,
    folders,
    nestedInside,
    worktreesInside,
  } = summary
  // Closes itself once every checked path has left the unstaged list, rather
  // than lingering with copy about files that are no longer there.
  const isOpen =
    useVanishedTargetGuard(open, targets.length + leftOut.length > 0, onCancel) &&
    changed === null

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
    if (worktreesInside > 0) {
      sentences.push(
        `The ${countWords(
          worktreesInside,
          "worktree of this repository",
          "worktrees of this repository",
        )} inside them ${worktreesInside === 1 ? "is" : "are"} kept.`,
      )
    }
  }
  // The rows left out name their folders, so they are prose with chips.
  const leftOutSentence: Prose | null =
    leftOut.length > 0
      ? prose`${formatRegularCount(leftOut.length, "selected row")} ${
          leftOut.length === 1 ? "is" : "are"
        } left out: ${joinProse(leftOut, "; ")}.`
      : null
  const bareKept = bareRepositoriesKept(bare, filesInBare, "them")
  const closing = "This action cannot be undone."
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
        <p className="text-sm text-destructive">
          {body}
          {bareKept && <> {renderProse(bareKept)}</>}
          {leftOutSentence && <> {renderProse(leftOutSentence)}</>} {closing}
        </p>
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
            onClick={() =>
              onConfirm(
                targets.map((f) => f.path),
                Object.fromEntries(
                  targets.flatMap((f) => (f.kind ? [[f.path, discardConfirmation(f)]] : [])),
                ),
              )
            }
          >
            Discard
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
