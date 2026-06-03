import { Sparkles } from "lucide-react"
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
  socket,
  useDux,
} from "@/lib/store"

export function CommitDialog() {
  const { commitTarget, commitDraft, viewModel } = useDux()

  const isOpen = commitTarget !== null
  const stagedCount = viewModel?.changed_files.staged.length ?? 0

  function handleCommit() {
    if (!commitTarget || !commitDraft.trim()) return
    socket.sendCommand("commit_changes", {
      session_id: commitTarget,
      message: commitDraft.trim(),
    })
    closeCommit()
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
            <Button onClick={handleCommit} disabled={!commitDraft.trim()}>
              Commit
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
