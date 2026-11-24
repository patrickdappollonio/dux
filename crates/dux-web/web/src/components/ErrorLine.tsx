import type { ReactNode } from "react"

import { renderMarked } from "@/lib/prose"

// The red sentence under a field, or in a dialog's body, that says an action
// failed or an input is refused. It is a sentence, not a framed notice (that is
// `Notice`), and there is no prop for a class or a style: the colour is fixed
// here and the size is a named choice, so two error lines cannot drift apart.
//
// `announce` is what a screen reader is told when the line appears:
// - "alert" (the default) interrupts: for the answer to something the user just
//   did (a failed save, a refused sign-in, a load that failed, a refusal after
//   picking a destination). A role=alert region that is created together with
//   its words is read.
// - "silent" is no live region at all. For a refusal that follows what the user
//   types: the field is marked `aria-invalid` and `aria-describedby` names this
//   line's `id`, so it is read with the field, and nothing is shouted over each
//   keystroke. (A role=status region inserted with its text is commonly not read
//   at all, which is why that is not the middle way here.) It is also for words
//   a caller has already said another way, such as a toast.
// A line that is a standing warning about a destructive act, present when its
// dialog opens, is not an error and is not this component.
//
// `shape="log"` is for a parser's multi-line message: monospace, keeping its
// line breaks, and scrolling within a short box instead of growing the dialog.
//
// `size` matches the neighbouring text: "sm" is the default, "xs" for rows
// whose other text is `text-xs`.
//
// A sentence of dux's own words that marks a command or a setting with
// backticks (a refusal the server wrote) is drawn with the chip and without
// them (`renderMarked`). The log shape is a parser's own text, shown as it came.
//
// With no words the component renders nothing, so a caller can hand it its
// error as it is.
export type ErrorLineAnnounce = "alert" | "silent"
export type ErrorLineShape = "sentence" | "log"
export type ErrorLineSize = "sm" | "xs"

export function ErrorLine({
  children,
  announce = "alert",
  shape = "sentence",
  size = "sm",
  id,
  ...testing
}: {
  children?: ReactNode
  announce?: ErrorLineAnnounce
  shape?: ErrorLineShape
  size?: ErrorLineSize
  // What a field's aria-describedby points at.
  id?: string
  "data-testid"?: string
}) {
  if (isEmpty(children)) return null
  const text = size === "xs" ? "text-xs" : "text-sm"
  return (
    <p
      {...testing}
      id={id}
      role={announce === "alert" ? "alert" : undefined}
      className={
        shape === "log"
          ? `max-h-24 overflow-y-auto font-mono ${text} break-words whitespace-pre-wrap text-destructive`
          : `${text} text-destructive`
      }
    >
      {shape === "log" ? children : markChildren(children)}
    </p>
  )
}

function isEmpty(children: ReactNode): boolean {
  if (Array.isArray(children)) return children.every(isEmpty)
  return children == null || children === false || children === ""
}

function markChildren(children: ReactNode): ReactNode {
  if (typeof children === "string") return renderMarked(children)
  if (Array.isArray(children)) {
    return children.map((child) => (typeof child === "string" ? renderMarked(child) : child))
  }
  return children
}
