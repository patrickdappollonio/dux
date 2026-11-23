import { CircleAlert, RotateCw, Trash2, X } from "lucide-react"

import { Notice, type NoticeAction } from "@/components/Notice"
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
  // Another operation is out on the file (lib/editorLane.ts); Reload would
  // start a second one, so it waits.
  busy?: boolean
}

// The in-pane notice that the file on disk is no longer what this buffer holds.
//
// Not a toast, because it is positional truth about one tab rather than an event:
// it must still be there on returning to the tab and gone on switching away. Not
// a modal, because the answer to a changed file is often to look at what you
// typed first, which a modal forbids.
//
// It appears only where the editor could not resolve the situation itself: a
// clean buffer reloads in place with no banner.
export function FileDiskBanner({
  state,
  path,
  dirty,
  onReload,
  onDismiss,
  onCloseTab,
  busy = false,
}: FileDiskBannerProps) {
  if (state === "fresh") return null
  const deleted = state === "deleted"
  // The clean-but-selecting rung. Nothing is at risk, so saying "you have
  // unsaved edits" here would be a plain untruth, and the next warning the
  // user reads is worth less for it.
  const paused = state === "paused"
  const actions: NoticeAction[] = [
    // A stray click on "Reload" discards text, so it never sits flush against
    // the dismiss: the notice spaces its buttons apart.
    deleted
      ? { label: "Close tab", onSelect: onCloseTab, icon: X }
      : { label: "Reload from disk", onSelect: onReload, icon: RotateCw, disabled: busy },
    { label: deleted ? "Keep it open" : "Keep mine", onSelect: onDismiss, look: "quiet" },
  ]
  return (
    <Notice shape="strip" tone="warning" icon={deleted ? Trash2 : CircleAlert} actions={actions}>
      {deleted ? (
        <>
          <span className="font-mono break-all">{path}</span> was deleted on
          disk. {dirty ? "Your unsaved edits are still here." : "The text here is the last copy the editor read."}
        </>
      ) : paused ? (
        <>
          <span className="font-mono break-all">{path}</span> changed on
          disk. A selection is active here, so the reload is paused.
        </>
      ) : (
        <>
          <span className="font-mono break-all">{path}</span> changed on
          disk, and you have unsaved edits here.
        </>
      )}
    </Notice>
  )
}
