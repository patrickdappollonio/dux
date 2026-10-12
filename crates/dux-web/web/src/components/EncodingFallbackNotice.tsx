import { CircleAlert, X } from "lucide-react"

import { Button } from "@/components/ui/button"
import { InlineCode } from "@/components/ui/inline-code"

interface EncodingFallbackNoticeProps {
  path: string
  // The short name the status bar uses, e.g. "Windows-1252".
  encodingLabel: string
  // On a phone the encoding menu lives in the editor's ⋯ menu, since the phone
  // status bar holds no controls, so the sentence points there instead.
  phone: boolean
  onDismiss: () => void
}

// The in-pane notice that a file was opened in the fallback encoding: its bytes
// are not UTF-8 and nothing in the file says what they are, so the server read
// it as windows-1252, which keeps every byte but may show the wrong characters.
//
// The same family as the disk-changed banner (amber, in the pane, above the
// text) because it is the same kind of fact: positional truth about this tab
// that the person may need to act on, and that must still be there when they
// come back to the tab. It does not block anything: the file is safe to edit
// and save as it is, so the answer is optional and the notice can be waved off.
export function EncodingFallbackNotice({
  path,
  encodingLabel,
  phone,
  onDismiss,
}: EncodingFallbackNoticeProps) {
  return (
    <div
      // A live region: it appears on opening a file, unasked.
      role="status"
      className="flex shrink-0 items-center gap-2 border-b border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm"
    >
      <CircleAlert className="size-4 shrink-0 text-amber-500" />
      <span className="min-w-0 flex-1">
        <InlineCode>{path}</InlineCode> isn&rsquo;t valid UTF-8, so it opened
        as {encodingLabel}. If the text looks wrong, pick its encoding from{" "}
        {phone ? "the ⋯ menu" : "the status bar"}.
      </span>
      {/* Ghost and icon-only: the one control here, with the sentence as its
          only neighbour, so a stray tap lands on text. A finger gets the
          40px floor. */}
      <Button
        size="icon"
        variant="ghost"
        className="size-7 shrink-0 pointer-coarse:size-10"
        aria-label="Dismiss the encoding notice"
        onClick={onDismiss}
      >
        <X />
      </Button>
    </div>
  )
}
