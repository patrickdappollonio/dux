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
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetTitle,
} from "@/components/ui/sheet"
import { useIsMobile } from "@/hooks/use-mobile"
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
import { cn } from "@/lib/utils"

// The field and the two buttons share one height: 44px, the height a dialog
// footer's buttons take on a phone, wherever a finger is what presses them.
const PHONE_HEIGHT = "h-11"
const DESKTOP_HEIGHT = "pointer-coarse:h-11"

// Attach a pull request to an agent, beside the ones it has. One text field for
// the raw reference; the lookup runs server-side and its outcome rides the
// status toast stream, so submitting closes the dialog at once. The body says
// how many the agent has, because attaching adds rather than replaces. A
// dialog on a computer and a bottom sheet on a phone, like the Pull requests
// dialog it is opened from.
export function AttachPullRequestDialog() {
  const { attachPullRequestTarget, attachPullRequestDraft, spine } = useDux()
  const isMobile = useIsMobile()
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

  const height = isMobile ? PHONE_HEIGHT : DESKTOP_HEIGHT
  const onOpenChange = (o: boolean) => {
    if (!o) closeAttachPullRequest()
  }
  const Title = isMobile ? SheetTitle : DialogTitle
  const Description = isMobile ? SheetDescription : DialogDescription
  const heading = (
    <>
      <Title>Attach pull request</Title>
      <Description>
        Adds a GitHub pull request to <InlineCode>{name}</InlineCode>,
        beside the ones found on its branches. Autodetection keeps running.
      </Description>
    </>
  )
  const fields = (
    <>
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
        className={height}
        autoFocus
      />
    </>
  )
  const buttons = (
    <>
      <Button
        variant="outline"
        className={cn(height, isMobile && "flex-1")}
        onClick={closeAttachPullRequest}
      >
        Cancel
      </Button>
      <Button
        className={cn(height, isMobile && "flex-1")}
        onClick={handleSubmit}
        disabled={empty}
      >
        Attach
      </Button>
    </>
  )

  if (isMobile) {
    return (
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetContent
          side="bottom"
          className="gap-3 rounded-t-2xl p-4 pb-[max(env(safe-area-inset-bottom),1rem)]"
        >
          <div className="flex flex-col gap-1 pr-10">{heading}</div>
          {fields}
          <div className="flex gap-2 pt-2">{buttons}</div>
        </SheetContent>
      </Sheet>
    )
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>{heading}</DialogHeader>
        {fields}
        <div className="h-2" />
        <DialogFooter>{buttons}</DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
