import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { InlineCode } from "@/components/ui/inline-code"
import { Input } from "@/components/ui/input"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
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
  const { attachPullRequestTarget, attachPullRequestDraft, spine } = useDux()
  const session = spine?.sessions.find((s) => s.id === attachPullRequestTarget)
  // Closes the dialog when the agent vanishes from the ViewModel: attaching a
  // PR to a deleted agent is moot. See the hook.
  const open = useVanishedTargetGuard(
    attachPullRequestTarget !== null,
    session !== undefined,
    closeAttachPullRequest,
  )

  const name = session ? sessionLabel(session) : ""
  const count = session ? sessionPrs(session).length : 0
  const empty = attachPullRequestDraft.trim() === ""

  function handleSubmit() {
    if (empty) return
    submitAttachPullRequest()
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) closeAttachPullRequest()
      }}
    >
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Attach pull request</DialogTitle>
          <DialogDescription>
            Adds a GitHub pull request to <InlineCode>{name}</InlineCode>,
            beside the ones found on its branches. Autodetection keeps running.
          </DialogDescription>
        </DialogHeader>
        <p className="text-sm text-muted-foreground">
          <InlineCode>{name}</InlineCode>{" "}
          {count === 0
            ? "has no pull requests yet."
            : `has ${formatRegularCount(count, "pull request")} now.`}{" "}
          One from another repository works too.
        </p>
        <Input
          value={attachPullRequestDraft}
          onChange={(e) => setAttachPullRequestDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault()
              handleSubmit()
            }
          }}
          placeholder="PR URL, #123, or 123"
          autoFocus
        />
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" onClick={closeAttachPullRequest}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={empty}>
            Attach
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
