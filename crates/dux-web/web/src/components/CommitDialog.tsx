import { useState } from "react"
import { Loader2 } from "lucide-react"
import { notifyError, notifySuccess } from "@/lib/notify"
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
import { closeCommit, getSnapshot, setCommitDraft, useDux } from "@/lib/store"

export function CommitDialog() {
  const { commitTarget, commitDraft } = useDux()
  const [committing, setCommitting] = useState(false)

  const isOpen = commitTarget !== null

  async function handleCommit() {
    if (!commitTarget || !commitDraft.trim() || committing) return
    // What this commit was sent from: the dialog's opening, its agent and the
    // exact text in the box.
    const sentFrom = {
      target: commitTarget,
      opening: getSnapshot().commitOpening,
      draft: commitDraft,
    }
    setCommitting(true)
    try {
      await git.commit(sentFrom.target, sentFrom.draft.trim())
      // The answer closes the dialog only while it is still the one the
      // commit was sent from, unchanged since: a dialog reopened (for this
      // agent or another) or a message typed since is never wiped. The toast
      // then says the commit landed instead.
      const now = getSnapshot()
      if (
        now.commitTarget === sentFrom.target &&
        now.commitOpening === sentFrom.opening &&
        now.commitDraft === sentFrom.draft
      ) {
        closeCommit()
      } else if (now.commitTarget === null) {
        // The dialog was cancelled while the commit ran: there is no message
        // being written to speak of.
        notifySuccess("The commit landed.")
      } else {
        notifySuccess(
          "The earlier commit landed. The message you are writing now was left as it is.",
        )
      }
    } catch (err) {
      notifyError(err instanceof Error ? err.message : "commit failed")
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
