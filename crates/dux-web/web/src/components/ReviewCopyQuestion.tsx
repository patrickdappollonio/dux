import { TriangleAlert } from "lucide-react"

import { CheckboxRow } from "@/components/CheckboxRow"
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
      <p
        role="alert"
        className="flex items-start gap-2 rounded-md border border-dux-warning px-3 py-2 text-sm"
      >
        <TriangleAlert className="mt-0.5 size-4 shrink-0 text-dux-warning" />
        <span>{renderProse(warning)}</span>
      </p>
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
