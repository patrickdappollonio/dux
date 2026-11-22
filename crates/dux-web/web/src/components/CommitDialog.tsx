import { useId, useState } from "react"
import { notifyError, notifySuccess } from "@/lib/notify"
import { git } from "@/lib/git"
import { Modal } from "@/components/modal"
import { Textarea } from "@/components/ui/textarea"
import {
  agentChangeBlockReason,
  useRowDriverLabel,
  useAgentChangeBlockById,
} from "@/hooks/use-agent-active-elsewhere"
import { closeCommit, getSnapshot, setCommitDraft, useDux } from "@/lib/store"

export function CommitDialog() {
  const { commitTarget, commitDraft } = useDux()
  const [committing, setCommitting] = useState(false)
  const reasonId = useId()
  // A page that does not drive the agent keeps the message it was writing,
  // read-only, and gets it back to commit once it takes over.
  const block = useAgentChangeBlockById(commitTarget ?? "")
  const device = useRowDriverLabel(commitTarget === null ? null : { sessionId: commitTarget })
  const reason =
    commitTarget !== null && block !== null ? agentChangeBlockReason(block, device) : null
  const blocked = reason !== null

  const isOpen = commitTarget !== null

  async function handleCommit() {
    if (!commitTarget || !commitDraft.trim() || committing || blocked) return
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

  return (
    <Modal
      kind="form"
      open={isOpen}
      onClose={() => closeCommit()}
      title="Commit changes"
      submit={{
        label: "Commit",
        tone: "primary",
        onSelect: () => void handleCommit(),
        busy: committing,
        disabledReason: reason ?? (commitDraft.trim() ? null : "Write a commit message first."),
      }}
    >
      <Textarea
        placeholder="Commit message…"
        value={commitDraft}
        onChange={(e) => setCommitDraft(e.target.value)}
        className="min-h-24 resize-none"
        readOnly={blocked}
        aria-describedby={blocked ? reasonId : undefined}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault()
            void handleCommit()
          }
        }}
      />
      {blocked ? (
        <p id={reasonId} className="text-sm text-muted-foreground">
          {reason}
        </p>
      ) : null}
    </Modal>
  )
}
