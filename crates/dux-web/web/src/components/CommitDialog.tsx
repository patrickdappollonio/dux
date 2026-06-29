import { useState } from "react"
import { Loader2 } from "lucide-react"
import { toast } from "sonner"
import { git } from "@/lib/git"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Textarea } from "@/components/ui/textarea"
import { closeCommit, setCommitDraft, useDux } from "@/lib/store"

export function CommitDialog() {
  const { commitTarget, commitDraft } = useDux()
  const [committing, setCommitting] = useState(false)

  const isOpen = commitTarget !== null

  async function handleCommit() {
    if (!commitTarget || !commitDraft.trim() || committing) return
    setCommitting(true)
    try {
      await git.commit(commitTarget, commitDraft.trim())
      closeCommit()
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "commit failed")
    } finally {
      setCommitting(false)
    }
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
        <DialogFooter>
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
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
