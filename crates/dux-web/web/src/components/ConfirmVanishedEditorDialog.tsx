import { Modal } from "@/components/modal"
import {
  discardVanishedEditor,
  keepVanishedEditor,
  useDux,
} from "@/lib/store"

// What an editor says when the thing it was rooted at goes away while a buffer
// is dirty.
//
// A sibling of `ConfirmCloseEditorTabDialog` rather than a variant of the reload
// confirm, which is about disk truth: the root that could have saved this text is
// what vanished, so there is no Save to offer and no reload to run. The choice is
// between leaving and staying to copy the words out, so keeping it open is the
// safe default and the destructive half discards.
//
// Both surfaces raise it, through the store's one `endOpenEditorIfRootGone`
// handler, and on either only this confirm may discard the text; a clean buffer
// closes with a warning toast instead.
export function ConfirmVanishedEditorDialog() {
  const { editorTargetGone } = useDux()
  const isTerminal = editorTargetGone?.kind === "terminal"

  return (
    <Modal
      kind="confirm"
      open={editorTargetGone !== null}
      // Every way out but the destructive button is the safe half here, unlike
      // the per-tab close confirm: the text stays on screen and nothing is lost.
      onClose={keepVanishedEditor}
      // Raised by the target vanishing, never by a click, so it takes no history
      // entry for the browser's Back.
      historyEntry={false}
      title={
        isTerminal
          ? "That terminal closed while you were editing"
          : "That agent is gone while you were editing"
      }
      cancelLabel="Keep it open"
      confirm={{ label: "Discard & leave", tone: "destructive", onSelect: discardVanishedEditor }}
    >
      <p className="text-sm text-destructive">
        {isTerminal
          ? "The editor was rooted at the directory that terminal started in, and that root went with it, so your unsaved edits can no longer be saved from here."
          : "The editor was rooted at that agent's worktree, and it went with it, so your unsaved edits can no longer be saved from here."}{" "}
        Keep this open if you want to copy the text out first.
      </p>
    </Modal>
  )
}
