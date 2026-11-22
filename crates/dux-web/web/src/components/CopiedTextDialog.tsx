import { useRef } from "react"

import { Modal } from "@/components/modal"
import { InlineCode } from "@/components/ui/inline-code"
import { Textarea } from "@/components/ui/textarea"
import { closeCopyText, useCopyText } from "@/lib/agentCopy"

// The last way to a copy the browser would not take: the text itself, selected,
// for the user to copy with their own keys or a long press. Read-only, so the
// text cannot be edited by accident, and a phone raises no keyboard over it,
// which is why it takes focus on open under a finger too.
export function CopiedTextDialog() {
  const view = useCopyText()
  const text = useRef<HTMLTextAreaElement | null>(null)

  return (
    <Modal
      kind="report"
      open={view !== null}
      onClose={closeCopyText}
      size="lg"
      initialFocus={text}
      title={<>Text copied from {view ? <InlineCode>{view.source}</InlineCode> : "a pane"}</>}
      description="Your browser would not let dux put this on your clipboard. It is selected below: copy it with your keyboard, or press and hold it on a touch screen."
    >
      <Textarea
        ref={text}
        aria-label="Copied text"
        readOnly
        value={view?.text ?? ""}
        onFocus={(event) => event.currentTarget.select()}
        className="max-h-[50dvh] overflow-auto font-mono"
      />
    </Modal>
  )
}
