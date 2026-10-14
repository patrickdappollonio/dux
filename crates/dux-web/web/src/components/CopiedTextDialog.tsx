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
import { Textarea } from "@/components/ui/textarea"
import { closeCopyText, useCopyText } from "@/lib/agentCopy"

// The last way to a copy the browser would not take: the text itself, selected,
// for the user to copy with their own keys or a long press. Read-only, so the
// text cannot be edited by accident, and a phone raises no keyboard over it.
export function CopiedTextDialog() {
  const view = useCopyText()

  return (
    <Dialog
      open={view !== null}
      onOpenChange={(open) => {
        if (!open) closeCopyText()
      }}
    >
      <DialogContent showCloseButton={false} className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            Text copied from {view ? <InlineCode>{view.source}</InlineCode> : "a pane"}
          </DialogTitle>
          <DialogDescription>
            Your browser would not let dux put this on your clipboard. It is
            selected below: copy it with your keyboard, or press and hold it on a
            touch screen.
          </DialogDescription>
        </DialogHeader>
        <Textarea
          aria-label="Copied text"
          readOnly
          autoFocus
          value={view?.text ?? ""}
          onFocus={(event) => event.currentTarget.select()}
          className="max-h-[50dvh] overflow-auto font-mono"
        />
        <DialogFooter>
          <Button variant="outline" onClick={closeCopyText}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
