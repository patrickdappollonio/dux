import { TriangleAlert } from "lucide-react"

import { CheckboxRow } from "@/components/CheckboxRow"
import { Notice } from "@/components/Notice"
import { type Prose, renderProse } from "@/lib/prose"

// The server will not make a review copy nobody agreed to. A push from the
// agent would land on the copy, not on the pull request, so the warning stays
// in view and the box starts unticked. Without `onAgreedChange` there is
// nothing to agree to (every name the copy could take is taken), so only the
// sentence shows.
export function ReviewCopyQuestion({
  warning,
  agreed,
  onAgreedChange,
}: {
  warning: Prose
  agreed?: boolean
  onAgreedChange?: (agreed: boolean) => void
}) {
  return (
    <>
      <Notice shape="box" tone="warning" icon={TriangleAlert}>
        {renderProse(warning)}
      </Notice>
      {onAgreedChange && (
        <CheckboxRow
          id="create-review-copy"
          checked={agreed ?? false}
          onCheckedChange={onAgreedChange}
        >
          Create a review copy
        </CheckboxRow>
      )}
    </>
  )
}
