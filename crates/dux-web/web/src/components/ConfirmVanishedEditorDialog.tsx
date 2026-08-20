import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  discardVanishedEditor,
  keepVanishedEditor,
  useDux,
} from "@/lib/store"

// What an editor says when the thing it was rooted at goes away while a
// buffer is dirty.
//
// It is a sibling of `ConfirmCloseEditorTabDialog`, not a variant of the reload
// confirm: that one is about disk truth (the file moved, reload or keep), and
// this one is about the ROOT, which has no disk truth left to compare against.
// The root that could have saved this text is exactly what vanished, so there
// is no Save to offer and no reload to run. The honest choice is between
// leaving and staying to copy the words out, so "Keep it open" is the safe
// default and the destructive half is the one that discards.
//
// Both surfaces raise it: the standalone tab and the in-app overlay share one
// vanish handler (`endOpenEditorIfRootGone` in the store), because the
// workspace still being visible around the overlay changes nothing about the
// unsaved text. On either surface, only this confirm may discard it; a clean
// buffer closes with a warning toast instead.
export function ConfirmVanishedEditorDialog() {
  const { editorTargetGone } = useDux()
  const isTerminal = editorTargetGone?.kind === "terminal"

  function handleOpenChange(open: boolean) {
    // Dismissing is the SAFE half here, unlike the per-tab close confirm: the
    // text stays on screen either way, and nothing has been discarded.
    if (!open) keepVanishedEditor()
  }

  return (
    <Dialog open={editorTargetGone !== null} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>
            {isTerminal
              ? "That terminal closed while you were editing"
              : "That agent is gone while you were editing"}
          </DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          {isTerminal
            ? "The editor was rooted at the directory that terminal started in, and that root went with it, so your unsaved edits can no longer be saved from here."
            : "The editor was rooted at that agent's worktree, and it went with it, so your unsaved edits can no longer be saved from here."}{" "}
          Keep this open if you want to copy the text out first.
        </p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={keepVanishedEditor}>
            Keep it open
          </Button>
          <Button variant="destructive" onClick={discardVanishedEditor}>
            Discard & leave
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
