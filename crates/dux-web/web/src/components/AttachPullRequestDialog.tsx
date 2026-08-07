import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import {
  closeAttachPullRequest,
  setAttachPullRequestDraft,
  submitAttachPullRequest,
  useDux,
} from "@/lib/store"

// Manually attach (pin) a GitHub pull request to an agent. One text field for
// the raw reference; the lookup runs server-side and its outcome rides the
// status toast stream, so submitting closes the dialog immediately (modeled on
// the rename dialog's shape, but deferred like the other 202-style actions).
// When the agent already shows a PR the body names it, so replacing it is an
// explicit, informed act rather than a surprise.
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

  const pr = session?.pr
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
            Pins a GitHub pull request to this agent and pauses branch-name
            autodetection until you detach it.
          </DialogDescription>
        </DialogHeader>
        {pr ? (
          <p className="text-sm text-muted-foreground">
            Currently showing{" "}
            <span className="text-foreground">
              #{pr.number} {pr.title}
            </span>
            {pr.overridden ? " (manually attached)" : ""}. Attaching replaces
            it.
          </p>
        ) : null}
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
