import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { countWords } from "@/lib/changedFiles"
import { closeDiscard, discardFile, useDux } from "@/lib/store"

// Confirmation before discarding an unstaged file's changes, which cannot be
// undone. The copy distinguishes the outcomes: a tracked file is restored from its
// last committed state, an untracked file is permanently DELETED.
export function ConfirmDiscardFileDialog() {
  const { discardTarget, changes } = useDux()

  // Trust the changes slice only when it belongs to the discard target's session.
  const stillUnstaged =
    discardTarget !== null &&
    changes.sessionId === discardTarget.sessionId &&
    changes.unstaged.some((f) => f.path === discardTarget.path)
  // Closes when the file leaves the unstaged list, rather than lingering on a
  // stale path whose restore-versus-DELETE copy may now be wrong.
  const isOpen = useVanishedTargetGuard(
    discardTarget !== null,
    stillUnstaged,
    closeDiscard,
  )
  const path = discardTarget?.path ?? ""
  const untracked = discardTarget?.untracked ?? false
  // A folded folder is deleted whole and a repository of its own with its
  // history, so the wording comes from the live row, not from the target.
  const row = stillUnstaged
    ? changes.unstaged.find((f) => f.path === discardTarget.path)
    : undefined
  const kind = row?.kind

  function handleConfirm() {
    if (!discardTarget) return
    // The live row says what went; a target whose row has just left the list
    // is worded from what the dialog was opened on.
    discardFile(
      discardTarget.sessionId,
      row ?? {
        path: discardTarget.path,
        status: discardTarget.untracked ? "??" : "M",
        additions: 0,
        deletions: 0,
        binary: false,
        diff_excluded: false,
      },
    )
    closeDiscard()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeDiscard()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>
            {kind === "directory" ? (
              <>
                Delete the untracked folder <InlineCode>{`${path}/`}</InlineCode>?
              </>
            ) : kind === "nested_repository" ? (
              <>
                Delete <InlineCode>{`${path}/`}</InlineCode>?
              </>
            ) : (
              <>
                Discard changes to <InlineCode>{path}</InlineCode>?
              </>
            )}
          </DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          {kind === "directory" ? (
            <>
              The {countWords(row?.file_count ?? 0, "file", "files")} inside{" "}
              <InlineCode>{`${path}/`}</InlineCode> will be permanently DELETED from disk.
              Files the repository ignores inside it are kept.
              {(row?.nested_repositories ?? 0) > 0 && (
                <>
                  {" "}
                  The{" "}
                  {countWords(
                    row?.nested_repositories ?? 0,
                    "nested repository",
                    "nested repositories",
                  )}{" "}
                  inside it {row?.nested_repositories === 1 ? "is" : "are"} kept.
                </>
              )}
              {(row?.linked_worktrees ?? 0) > 0 && (
                <>
                  {" "}
                  The{" "}
                  {countWords(
                    row?.linked_worktrees ?? 0,
                    "worktree of this repository",
                    "worktrees of this repository",
                  )}{" "}
                  inside it {row?.linked_worktrees === 1 ? "is" : "are"} kept.
                </>
              )}{" "}
              This action cannot be undone.
            </>
          ) : kind === "nested_repository" ? (
            <>
              <InlineCode>{`${path}/`}</InlineCode> is a repository of its own. Deleting
              it removes the whole repository, including its history and any commits not
              pushed anywhere else. This action cannot be undone.
            </>
          ) : untracked ? (
            <>
              <InlineCode>{path}</InlineCode> is untracked and will be{" "}
              permanently DELETED from disk. This action cannot be undone.
            </>
          ) : (
            <>
              All changes to <InlineCode>{path}</InlineCode> will be{" "}
              restored to its last committed state. This action cannot be undone.
            </>
          )}
        </p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          {/* Cancel is the default focus, matching the TUI (Cancel highlighted).
              shadcn/radix buttons already activate on Space/Enter natively. */}
          <Button variant="outline" autoFocus onClick={closeDiscard}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            {kind ? "Delete" : "Discard"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
