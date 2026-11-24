import { useEffect, useId, useRef, type ReactNode } from "react"

import { ErrorLine, type ErrorLineAnnounce } from "@/components/ErrorLine"
import { FIELD_HELP_TEXT, FIELD_LABEL_TEXT } from "@/components/ui/form-text"
import { useIsMobile } from "@/hooks/use-mobile"
import { cn } from "@/lib/utils"

/** What the control spreads onto itself, so it is named and described by the group. */
export interface FieldWiring {
  id?: string
  "aria-label"?: string
  "aria-labelledby"?: string
  "aria-describedby"?: string
  "aria-invalid"?: true
}

// A field is named by words drawn above or beside it, or, with none drawn, by a
// name only a screen reader says.
type FieldName = { label: ReactNode; spokenName?: never } | { spokenName: string; label?: never }

// "side" puts the words on the left and the control on the right; a "wide"
// control goes under the words on a phone. "stacked" draws the words, then the
// control, then its note; "fill" makes the group and its control take the
// height left in a column.
type FieldLayout =
  | { layout: "side"; width?: "compact" | "wide"; height?: never }
  | { layout: "stacked"; height?: "content" | "fill"; width?: never }

export type FormGroupProps = FieldName &
  FieldLayout & {
    // "single" is one control a `<label>` points at; "group" is a set of
    // controls (a radio group) named by the label's id.
    control: "single" | "group"
    help?: ReactNode
    // About the current value: a scope, what the chosen option means.
    note?: ReactNode
    error?: ReactNode
    errorAnnounce?: ErrorLineAnnounce
    // Ids of lines drawn elsewhere that also describe the control.
    describedBy?: string
    invalid?: boolean
    // The words a screen reader hears for the dot that marks a changed value.
    changed?: string | null
    // Controls on the field's row that act on its value: `before` it (reset,
    // undo) and `after` it (a `⋯`).
    before?: ReactNode
    after?: ReactNode
    children: (field: FieldWiring) => ReactNode
  }

/** One labelled field: its words, its control and how the two are tied, in one of two layouts. */
export function FormGroup(props: FormGroupProps) {
  const { control, help, note, error, errorAnnounce = "silent", describedBy, invalid, changed, before, after, children } =
    props
  const phone = useIsMobile()
  const base = useId()
  const id = `${base}control`
  const labelId = `${base}label`
  const helpId = help != null ? `${base}help` : undefined
  const noteId = note != null ? `${base}note` : undefined
  const hasError = error != null && error !== false && error !== ""
  const errorId = hasError ? `${base}error` : undefined
  const spoken = props.spokenName !== undefined
  const box = useRef<HTMLDivElement>(null)

  const field: FieldWiring = {
    id: control === "single" ? id : undefined,
    "aria-label": spoken ? props.spokenName : undefined,
    "aria-labelledby": control === "group" && !spoken ? labelId : undefined,
    "aria-describedby": [helpId, noteId, errorId, describedBy].filter(Boolean).join(" ") || undefined,
    "aria-invalid": invalid || hasError ? true : undefined,
  }

  useEffect(() => {
    if (!import.meta.env.DEV || box.current === null) return
    if (!wired(box.current, field)) {
      console.error("FormGroup: the control does not carry the wiring it was handed; spread it onto the control.")
    }
  })

  // The group's own tab stop, which is its chosen option in a roving group.
  function focusGroup() {
    const group = Array.from(box.current?.querySelectorAll("[aria-labelledby]") ?? []).find((el) =>
      el.getAttribute("aria-labelledby")?.split(/\s+/).includes(labelId),
    )
    const stop = group?.querySelector<HTMLElement>('[tabindex="0"]')
    stop?.focus()
  }

  const labelEl = spoken ? null : control === "single" ? (
    <label id={labelId} htmlFor={id} className={cn(FIELD_LABEL_TEXT, "min-w-0")}>
      {props.label}
    </label>
  ) : (
    <span id={labelId} onClick={focusGroup} className={cn(FIELD_LABEL_TEXT, "min-w-0")}>
      {props.label}
    </span>
  )
  const labelLine =
    labelEl !== null && changed ? (
      <div className="flex items-center gap-1.5">
        <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-primary" />
        <span className="sr-only">{changed}</span>
        {labelEl}
      </div>
    ) : (
      labelEl
    )
  const helpEl = helpId ? (
    <div id={helpId} className={FIELD_HELP_TEXT}>
      {help}
    </div>
  ) : null
  const noteEl = noteId ? (
    <div id={noteId} className={FIELD_HELP_TEXT}>
      {note}
    </div>
  ) : null
  const errorEl = (
    <ErrorLine announce={errorAnnounce} size="xs" id={errorId}>
      {hasError ? error : null}
    </ErrorLine>
  )
  const controlEl = children(field)

  if (props.layout === "side") {
    const stacked = phone && props.width === "wide"
    return (
      <div
        ref={box}
        data-form-layout={stacked ? "stacked" : "side"}
        className={cn("flex gap-3", stacked ? "flex-col" : "items-start")}
      >
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          {labelLine}
          {helpEl}
          {noteEl}
          {errorEl}
        </div>
        <div data-form-control className={cn("flex items-center gap-1", stacked ? "w-full" : "shrink-0")}>
          {before}
          {controlEl}
          {after}
        </div>
      </div>
    )
  }

  const fill = props.height === "fill"
  const header =
    labelLine !== null && helpEl !== null ? (
      <div className="flex flex-col gap-1">
        {labelLine}
        {helpEl}
      </div>
    ) : (
      (labelLine ?? helpEl)
    )
  // With nothing on its row and no height to take, the control is a direct
  // child of the column, so its own width classes behave as they would alone.
  const row =
    before != null || after != null || fill ? (
      <div
        data-form-control
        className={cn("flex min-w-0 gap-2", fill ? "flex-1 items-stretch" : "items-center")}
      >
        {before}
        {controlEl}
        {after}
      </div>
    ) : (
      controlEl
    )
  return (
    <div ref={box} data-form-layout="stacked" className={cn("flex flex-col gap-2", fill && "min-h-0 flex-1")}>
      {header}
      <div className={cn("flex flex-col gap-1.5", fill && "flex-1")}>
        {row}
        {noteEl}
        {errorEl}
      </div>
    </div>
  )
}

// Whether the control under `root` carries what it was handed: a single
// control its id, a group its label.
function wired(root: HTMLElement, field: FieldWiring): boolean {
  if (field.id !== undefined) return root.contains(root.ownerDocument.getElementById(field.id))
  const attr = field["aria-label"] !== undefined ? "aria-label" : "aria-labelledby"
  const want = field[attr]
  return Array.from(root.querySelectorAll(`[${attr}]`)).some((el) => el.getAttribute(attr) === want)
}
