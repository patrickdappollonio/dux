import { useId } from "react"

import { Modal, useHeldWhileClosed } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { FormGroup } from "@/components/ui/form-group"
import { Input } from "@/components/ui/input"
import { sessionLabel } from "@/lib/agentWorkspace"
import { formatRegularCount } from "@/lib/formatRegularCount"
import { sessionPrs } from "@/lib/pr"
import {
  closeAttachPullRequest,
  setAttachPullRequestDraft,
  submitAttachPullRequest,
  useDux,
} from "@/lib/store"

// Attach a pull request to an agent, beside the ones it has. One text field for
// the raw reference; the lookup runs server-side and its outcome rides the
// status toast stream, so submitting closes the dialog at once. The body says
// how many the agent has, because attaching adds rather than replaces.
export function AttachPullRequestDialog() {
  const live = useDux()
  const open = live.attachPullRequestTarget !== null
  // What the dialog showed when the close began, kept while the close animates.
  const attachPullRequestTarget = useHeldWhileClosed(open, live.attachPullRequestTarget)
  const attachPullRequestDraft = useHeldWhileClosed(open, live.attachPullRequestDraft)
  const spine = useHeldWhileClosed(open, live.spine)
  const session = spine?.sessions.find((s) => s.id === attachPullRequestTarget)

  const name = session ? sessionLabel(session) : ""
  const countId = useId()
  const count = session ? sessionPrs(session).length : 0
  const empty = attachPullRequestDraft.trim() === ""

  function handleSubmit() {
    if (empty) return
    submitAttachPullRequest()
  }

  return (
    <Modal
      kind="form"
      open={open}
      // Attaching a pull request to a deleted agent is moot.
      vanished={open && session === undefined}
      onClose={closeAttachPullRequest}
      title="Attach pull request"
      description={
        <>
          Adds a GitHub pull request to <InlineCode>{name}</InlineCode>, beside the ones found
          on its branches. Autodetection keeps running.
        </>
      }
      submit={{
        label: "Attach",
        tone: "primary",
        onSelect: handleSubmit,
        disabledReason: empty ? "Enter the pull request first." : null,
      }}
    >
      <p id={countId} className="text-sm text-muted-foreground">
        <InlineCode>{name}</InlineCode>{" "}
        {count === 0
          ? "has no pull requests yet."
          : `has ${formatRegularCount(count, "pull request")} now.`}{" "}
        One from another repository works too.
      </p>
      <FormGroup layout="stacked" control="single" spokenName="Pull request URL or number" describedBy={countId}>
        {(field) => (
          <Input
            {...field}
            value={attachPullRequestDraft}
            onChange={(e) => setAttachPullRequestDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault()
                handleSubmit()
              }
            }}
            placeholder="PR URL, #123, or 123"
            // 44px under a finger, the height of the footer's buttons there.
            className="pointer-coarse:h-11"
          />
        )}
      </FormGroup>
    </Modal>
  )
}
