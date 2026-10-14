import { CircleAlert, X } from "lucide-react"

import { Button } from "@/components/ui/button"
import type { EncodingNotice as Notice } from "@/lib/editorEncoding"
import { renderProse } from "@/lib/prose"

interface EncodingNoticeProps {
  notice: Notice
  onDismiss: () => void
}

// One in-pane notice about how the open file was read: the encoding was the
// fallback, the encoding cannot write the file back unchanged, the bytes now
// look like UTF-8, or the line endings are mixed. The wording and which ones
// apply are `encodingNotices` in lib/editorEncoding.ts.
//
// The same family as the disk-changed banner (amber, in the pane, above the
// text) because it is the same kind of fact: positional truth about this tab
// that decides what a save would do, told before anything is written, and
// still there when the person comes back to the tab.
export function EncodingNotice({ notice, onDismiss }: EncodingNoticeProps) {
  return (
    <div
      // A live region: it appears on opening a file, unasked.
      role="status"
      className="flex shrink-0 items-center gap-2 border-b border-dux-warning/40 bg-dux-warning/10 px-3 py-2 text-sm"
    >
      <CircleAlert className="size-4 shrink-0 text-dux-warning" />
      <span className="min-w-0 flex-1">{renderProse(notice.message)}</span>
      {notice.dismissable && (
        // Ghost and icon-only: the one control here, with the sentence as its
        // only neighbour, so a stray tap lands on text. A finger gets the
        // 40px floor.
        <Button
          size="icon"
          variant="ghost"
          className="size-7 shrink-0 pointer-coarse:size-10"
          aria-label="Dismiss this notice"
          onClick={onDismiss}
        >
          <X />
        </Button>
      )}
    </div>
  )
}
