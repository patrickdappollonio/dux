import { useId, useState } from "react"
import { notifyError, notifySuccess } from "@/lib/notify"
import { git } from "@/lib/git"
import { Modal, useHeldWhileClosed } from "@/components/modal"
import { FormGroup } from "@/components/ui/form-group"
import { Textarea } from "@/components/ui/textarea"
import {
  agentChangeBlockReason,
  useRowDriverLabel,
  useAgentChangeBlockById,
} from "@/hooks/use-agent-active-elsewhere"
import { closeCommit, commitLanded, getSnapshot, setCommitDraft, useDux } from "@/lib/store"

export function CommitDialog() {
  const { commitTarget: openTarget, commitDraft, spine } = useDux()
  const isOpen = openTarget !== null
  // The agent this open is about, kept while the close animates.
  const commitTarget = useHeldWhileClosed(isOpen, openTarget)
  const [committing, setCommitting] = useState(false)
  const reasonId = useId()
  // A page that does not drive the agent keeps the message it was writing,
  // read-only, and gets it back to commit once it takes over.
  const block = useAgentChangeBlockById(commitTarget ?? "")
  const device = useRowDriverLabel(commitTarget === null ? null : { sessionId: commitTarget })
  const reason =
    commitTarget !== null && block !== null ? agentChangeBlockReason(block, device) : null
  const blocked = reason !== null
  const vanished =
    isOpen && spine !== null && !spine.sessions.some((s) => s.id === openTarget)

  async function handleCommit() {
    if (!isOpen || !commitTarget || !commitDraft.trim() || committing || blocked) return
    // What this commit was sent from: its agent and the exact text in the box.
    const sentFrom = { target: commitTarget, draft: commitDraft }
    setCommitting(true)
    try {
      await git.commit(sentFrom.target, sentFrom.draft.trim())
      // The answer closes the dialog only while it is still open on the agent
      // the commit was sent from, with the message that was sent: a dialog
      // opened for another agent, or a message typed since, is never wiped.
      // The toast then says the commit landed instead, and a dialog cancelled
      // while the commit ran has no message being written to speak of.
      const now = getSnapshot()
      const stillOpen = now.commitTarget === sentFrom.target && now.commitDraft === sentFrom.draft
      commitLanded(sentFrom.target, sentFrom.draft)
      if (!stillOpen) {
        notifySuccess(
          now.commitTarget === null
            ? "The commit landed."
            : "The earlier commit landed. The message you are writing now was left as it is.",
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
      // An agent deleted while its Commit is open closes it.
      vanished={vanished}
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
      <FormGroup layout="stacked" control="single" spokenName="Commit message" describedBy={blocked ? reasonId : undefined}>
        {(field) => (
          <Textarea
            {...field}
            placeholder="Commit message…"
            value={commitDraft}
            onChange={(e) => setCommitDraft(e.target.value)}
            className="min-h-24 resize-none"
            readOnly={blocked}
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                e.preventDefault()
                void handleCommit()
              }
            }}
          />
        )}
      </FormGroup>
      {blocked ? (
        <p id={reasonId} className="text-sm text-muted-foreground">
          {reason}
        </p>
      ) : null}
    </Modal>
  )
}
