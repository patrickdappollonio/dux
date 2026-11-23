import type { ReactNode } from "react"

// The red sentence under a field, or in a dialog's body, that says an action
// failed or an input is refused. It is a sentence, not a framed notice (that is
// `Notice`), and there is no prop for a class or a style: the size and colour
// are fixed here so two error lines cannot drift apart.
//
// `announce` is what a screen reader is told when the line appears:
// - "alert" (the default) interrupts: for the answer to something the user just
//   did (a failed save, a refused sign-in, a load that failed).
// - "status" is read politely: for a refusal that follows what the user types,
//   which would be shouted over every keystroke as an alert.
// A line that is a standing warning about a destructive act, present when its
// dialog opens, is not an error and is not this component.
//
// `shape="log"` is for a parser's multi-line message: monospace, keeping its
// line breaks, and scrolling within a short box instead of growing the dialog.
//
// With no words the component renders nothing, so a caller can hand it its
// error as it is. The live region is created together with its words, which is
// what makes a screen reader read them.
export type ErrorLineAnnounce = "alert" | "status"
export type ErrorLineShape = "sentence" | "log"

export function ErrorLine({
  children,
  announce = "alert",
  shape = "sentence",
  id,
  ...testing
}: {
  children?: ReactNode
  announce?: ErrorLineAnnounce
  shape?: ErrorLineShape
  // What a field's aria-describedby points at.
  id?: string
  "data-testid"?: string
}) {
  if (children == null || children === false || children === "") return null
  return (
    <p
      {...testing}
      id={id}
      role={announce === "alert" ? "alert" : "status"}
      className={
        shape === "log"
          ? "max-h-24 overflow-y-auto font-mono text-sm break-words whitespace-pre-wrap text-destructive"
          : "text-sm text-destructive"
      }
    >
      {children}
    </p>
  )
}
