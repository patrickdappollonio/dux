import { CircleAlert, RotateCw, Trash2, X } from "lucide-react"

import { Button } from "@/components/ui/button"
import type { DiskState } from "@/lib/editorBuffers"

interface FileDiskBannerProps {
  // Which rung this is. "fresh" renders nothing, so the caller can hand the
  // buffer's state straight through without a second condition.
  state: DiskState
  path: string
  // True when the buffer has unsaved edits. Only the copy and the reload
  // route change: a dirty reload discards the user's text, so the caller
  // sends it through a destructive confirm first.
  dirty: boolean
  onReload: () => void
  onDismiss: () => void
  onCloseTab: () => void
}

// The in-pane notice that the file on disk is no longer what this buffer
// holds. Deliberately NOT a toast and NOT a modal.
//
// Not a toast, because this is positional truth about one tab rather than an
// event: it must still be there when the user comes back to the tab in a
// minute, and it must be gone when they switch to another one. Not a modal,
// because the honest answer to "the file changed" is often "let me look at
// what I typed first", and a modal forbids exactly that.
//
// It only appears when the editor could NOT resolve the situation on its own.
// A clean buffer reloads in place with no banner at all, which is what the
// user asked for by having no edits to lose.
export function FileDiskBanner({
  state,
  path,
  dirty,
  onReload,
  onDismiss,
  onCloseTab,
}: FileDiskBannerProps) {
  if (state === "fresh") return null
  const deleted = state === "deleted"
  return (
    <div
      // A live region: the banner appears without the user having done
      // anything, so a screen reader has to be told.
      role="status"
      className="flex shrink-0 flex-wrap items-center gap-2 border-b border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm"
    >
      {deleted ? (
        <Trash2 className="size-4 shrink-0 text-amber-500" />
      ) : (
        <CircleAlert className="size-4 shrink-0 text-amber-500" />
      )}
      <span className="min-w-0 flex-1">
        {deleted ? (
          <>
            <span className="font-mono break-all">{path}</span> was deleted on
            disk. {dirty ? "Your unsaved edits are still here." : "The text here is the last copy the editor read."}
          </>
        ) : (
          <>
            <span className="font-mono break-all">{path}</span> changed on
            disk, and you have unsaved edits here.
          </>
        )}
      </span>
      {/* gap-2 between the two actions is the misclick-safe spacing: a stray
          click on "Reload" discards text, so it must not sit flush against
          the dismiss. */}
      {deleted ? (
        <Button size="sm" variant="outline" onClick={onCloseTab}>
          <X />
          Close tab
        </Button>
      ) : (
        <Button size="sm" variant="outline" onClick={onReload}>
          <RotateCw />
          Reload from disk
        </Button>
      )}
      <Button size="sm" variant="ghost" onClick={onDismiss}>
        {deleted ? "Keep it open" : "Keep mine"}
      </Button>
    </div>
  )
}
