import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { useIsMobile } from "@/hooks/use-mobile"
import { closeEditor, useDux } from "@/lib/store"
import { rootKey } from "@/lib/editorRoot"
import { EditorBody } from "@/components/EditorBody"
import { swallowMissedFileDrop } from "@/lib/editorDrop"


// The overlay shell, desktop-only: it stands down entirely while this tab is the
// standalone editor, so the two can never both mount an EditorBody over the same
// files. The body is keyed by ROOT only, so opening a new file never remounts it
// and drops the tab list. Closing is non-destructive, drafts and tabs both
// surviving it; `ConfirmCloseEditorTabDialog` is the real discard.
export function EditorOverlay() {
  const { editorTarget, standaloneEditor } = useDux()
  const isMobile = useIsMobile()

  if (isMobile || standaloneEditor) return null

  return (
    <Dialog
      open={editorTarget !== null}
      onOpenChange={(open) => {
        if (!open) closeEditor()
      }}
    >
      <DialogContent
        showCloseButton={false}
        // The floor under the file tree's drop targets: a dropped file the browser
        // handles NAVIGATES the tab away, and the tree's rows stop propagation, so
        // this only ever sees the misses.
        onDragOver={swallowMissedFileDrop}
        onDrop={swallowMissedFileDrop}
        className="flex h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[min(80rem,calc(100%-2rem))]"
      >
        <DialogTitle className="sr-only">Code editor</DialogTitle>
        <DialogDescription className="sr-only">
          Browse, edit, and diff files in this worktree.
        </DialogDescription>
        {editorTarget && (
          <EditorBody
            key={rootKey(editorTarget.root)}
            root={editorTarget.root}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

