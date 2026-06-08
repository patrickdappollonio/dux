import { useState } from "react"
import { Loader2, Sparkles } from "lucide-react"
import { toast } from "sonner"
import { git } from "@/lib/git"
import { shouldShowChangedFiles } from "@/lib/changedFiles"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Textarea } from "@/components/ui/textarea"
import {
  closeCommit,
  generateCommitMessage,
  setCommitDraft,
  useDux,
} from "@/lib/store"

export function CommitDialog() {
  const { commitTarget, commitDraft, viewModel } = useDux()
  const [committing, setCommitting] = useState(false)

  const isOpen = commitTarget !== null
  // Only trust the global staged list when it belongs to THIS dialog's session
  // (the same cross-tab guard the changed-files pane uses), so "Generate with
  // AI" never lights up off another session's stale staged count.
  const watchedSessionId = viewModel?.changed_files.watched_session_id ?? null
  const stagedCount = shouldShowChangedFiles(watchedSessionId, commitTarget)
    ? (viewModel?.changed_files.staged.length ?? 0)
    : 0

  async function handleCommit() {
    if (!commitTarget || !commitDraft.trim() || committing) return
    setCommitting(true)
    try {
      await git.commit(commitTarget, commitDraft.trim())
      // Commit produces no changed-files row movement to confirm it, so unlike
      // stage/unstage this gets an explicit success toast.
      toast.success("Changes committed.")
      closeCommit()
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "commit failed")
    } finally {
      setCommitting(false)
    }
  }

  function handleGenerate() {
    if (!commitTarget) return
    generateCommitMessage(commitTarget)
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeCommit()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Commit changes</DialogTitle>
        </DialogHeader>
        <Textarea
          placeholder="Commit message…"
          value={commitDraft}
          onChange={(e) => setCommitDraft(e.target.value)}
          className="min-h-24 resize-none"
          autoFocus
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
              e.preventDefault()
              handleCommit()
            }
          }}
        />
        <DialogFooter className="sm:justify-between">
          <Button
            variant="outline"
            onClick={handleGenerate}
            disabled={stagedCount === 0}
          >
            <Sparkles />
            Generate with AI
          </Button>
          <div className="flex gap-2">
            <Button variant="outline" onClick={() => closeCommit()}>
              Cancel
            </Button>
            <Button
              onClick={handleCommit}
              disabled={committing || !commitDraft.trim()}
              aria-busy={committing}
            >
              {committing ? (
                <Loader2 className="motion-safe:animate-spin" />
              ) : null}
              Commit
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
