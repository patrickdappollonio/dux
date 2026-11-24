import { useRef } from "react"

import { Modal, useHeldWhileClosed } from "@/components/modal"
import { FormGroup } from "@/components/ui/form-group"
import { InlineCode } from "@/components/ui/inline-code"
import { Textarea } from "@/components/ui/textarea"
import { closeCopyText, useCopyText } from "@/lib/agentCopy"

// The last way to a copy the browser would not take: the text itself, selected,
// for the user to copy with their own keys or a long press. Read-only, so the
// text cannot be edited by accident, and a phone raises no keyboard over it,
// which is why it takes focus on open under a finger too.
export function CopiedTextDialog() {
  const live = useCopyText()
  // The text the dialog showed, kept while the close animates.
  const view = useHeldWhileClosed(live !== null, live)
  const text = useRef<HTMLTextAreaElement | null>(null)

  return (
    <Modal
      kind="report"
      open={live !== null}
      onClose={closeCopyText}
      size="lg"
      initialFocus={text}
      title={<>Text copied from {view ? <InlineCode>{view.source}</InlineCode> : "a pane"}</>}
      description="Your browser would not let dux put this on your clipboard. It is selected below: copy it with your keyboard, or press and hold it on a touch screen."
    >
      <FormGroup layout="stacked" control="single" spokenName="Copied text">
        {(field) => (
          <Textarea
            {...field}
            ref={text}
            readOnly
            value={view?.text ?? ""}
            onFocus={(event) => event.currentTarget.select()}
            className="max-h-[50dvh] overflow-auto font-mono"
          />
        )}
      </FormGroup>
    </Modal>
  )
}
