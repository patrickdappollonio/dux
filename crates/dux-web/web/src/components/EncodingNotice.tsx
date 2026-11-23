import { CircleAlert } from "lucide-react"

import { Notice } from "@/components/Notice"
import type { EncodingNotice as EncodingNoticeData } from "@/lib/editorEncoding"
import { renderProse } from "@/lib/prose"

interface EncodingNoticeProps {
  notice: EncodingNoticeData
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
    <Notice
      shape="strip"
      tone="warning"
      icon={CircleAlert}
      onDismiss={notice.dismissable ? onDismiss : undefined}
    >
      {renderProse(notice.message)}
    </Notice>
  )
}
