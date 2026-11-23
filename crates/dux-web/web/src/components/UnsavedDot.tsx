import { SimpleTooltip } from "@/components/SimpleTooltip"

// The editor's "this buffer has unsaved edits" mark: one glyph, one tooltip
// and one screen-reader phrase wherever a file name carries it.
export function UnsavedDot() {
  return (
    <>
      <SimpleTooltip content="Unsaved changes">
        <span className="shrink-0 text-primary" aria-hidden="true">
          ●
        </span>
      </SimpleTooltip>
      <span className="sr-only">unsaved changes</span>
    </>
  )
}
