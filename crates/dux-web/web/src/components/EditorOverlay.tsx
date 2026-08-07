import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { useIsMobile } from "@/hooks/use-mobile"
import { closeEditor, useDux } from "@/lib/store"
import { EditorBody } from "@/components/EditorBody"


// The overlay shell: owns the Dialog and is desktop-only — Monaco is poor on
// touch, and every entry point to the OVERLAY is gated to desktop. The one
// deliberate exception to "no editor on phones" is the STANDALONE surface
// (StandaloneEditor.tsx), which phones reach at its own address, best-effort.
// While this tab IS that surface the overlay stands down entirely, so the
// Dialog and the standalone shell can never both mount an EditorBody (two
// Monaco model sets and two buffer maps over the same files). The body is
// keyed by SESSION ONLY (not file/mode) so opening a new file (or a
// preview-replace) while the overlay is already open never remounts it and
// drops the tab list. Esc/backdrop/Close all close immediately: closing is
// NON-destructive now (drafts survive in lib/editorDrafts.ts and the tab
// list survives in the store), so the old overlay-close discard dialog is
// retired. The per-tab close confirm (`ConfirmCloseEditorTabDialog`) remains
// the real discard.
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
        className="flex h-[calc(100dvh-2rem)] w-[calc(100%-2rem)] max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[min(80rem,calc(100%-2rem))]"
      >
        <DialogTitle className="sr-only">Code editor</DialogTitle>
        <DialogDescription className="sr-only">
          Browse, edit, and diff files in this worktree.
        </DialogDescription>
        {editorTarget && (
          <EditorBody
            key={editorTarget.sessionId}
            sessionId={editorTarget.sessionId}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

