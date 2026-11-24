import { useEffect, useId, useRef, type ReactElement, type ReactNode } from "react"

import { ErrorLine, type ErrorLineAnnounce } from "@/components/ErrorLine"
import { FIELD_CODE_TEXT, FIELD_HELP_TEXT, FIELD_LABEL_TEXT, FIELD_WARNING_TEXT } from "@/components/ui/form-text"
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
// name only a screen reader says. Neither may be missing: a label is a string or
// an element, so a value that may be undefined, null or false does not compile.
// The compiler cannot see an empty string, so the group reports one when drawn.
type FieldName = { label: ReactElement | string; spokenName?: never } | { spokenName: string; label?: never }

// One line about the current value. A "warning" one reads as the warning it is;
// a "code" one is a config key, monospace and broken anywhere it must wrap.
export type NoteLine = { words: ReactNode; tone?: "warning"; code?: never } | { words: ReactNode; code: true; tone?: never }

// The layout decides where the note lines and the error go, never the width of
// the screen. "side" keeps all of a field's words (label, help, notes, error)
// in one block beside the control, and on a phone a "wide" control goes under
// that whole block, so the notes are above it. "stacked" draws the label and
// help, then the control, then the notes and the error under it; "fill" makes
// the group and its control take the height left in a column, and "compact"
// sets the label 4px above its control instead of 8px, for the fields of a `FormSection`.
type FieldLayout =
  | { layout: "side"; width?: "compact" | "wide"; height?: never; density?: never }
  | { layout: "stacked"; height?: "content" | "fill"; density?: "regular" | "compact"; width?: never }

export type FormGroupProps = FieldName &
  FieldLayout & {
    // "single" is one control a `<label>` points at, so a press on the words
    // reaches it; "inert" is one control named by the words' id, which a press
    // leaves alone; "group" is a set of controls (a radio group) named by that id.
    control: "single" | "inert" | "group"
    help?: ReactNode
    // About the current value (a scope, what the chosen option means), a line
    // each, in order, after the help. A line with no words is not drawn.
    notes?: readonly NoteLine[]
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
  const { control, help, notes, error, errorAnnounce = "silent", describedBy, invalid, changed, before, after, children } =
    props
  const phone = useIsMobile()
  const base = useId()
  const id = `${base}control`
  const labelId = `${base}label`
  const helpId = help != null ? `${base}help` : undefined
  const noteLines = drawnNotes(notes, base)
  const hasError = error != null && error !== false && error !== ""
  const errorId = hasError ? `${base}error` : undefined
  const spoken = props.spokenName !== undefined
  const box = useRef<HTMLDivElement>(null)

  const field: FieldWiring = {
    id: control === "single" ? id : undefined,
    "aria-label": spoken ? props.spokenName : undefined,
    "aria-labelledby": control !== "single" && !spoken ? labelId : undefined,
    "aria-describedby": [helpId, ...noteLines.map((line) => line.id), errorId, describedBy].filter(Boolean).join(" ") || undefined,
    "aria-invalid": invalid || hasError ? true : undefined,
  }

  useEffect(() => {
    if (!import.meta.env.DEV || box.current === null) return
    const named = spoken
      ? props.spokenName
      : box.current.ownerDocument.getElementById(labelId)?.textContent
    if (!named?.trim()) {
      console.error("FormGroup: the field has no name; give it a label or a spokenName that is not empty.")
    }
    const missing = unwired(box.current, field)
    if (missing.length > 0) {
      console.error(
        `FormGroup: the control does not carry ${missing.join(", ")} of the wiring it was handed; spread all of it onto the control.`,
      )
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
    <span id={labelId} onClick={control === "group" ? focusGroup : undefined} className={cn(FIELD_LABEL_TEXT, "min-w-0")}>
      {props.label}
    </span>
  )
  // Not `UnsavedDot` (the editor's glyph, with a tooltip): this dot says in words the value
  // differs from its default. A side field's label always sits in this row, as wide as its words.
  const labelLine =
    labelEl !== null && (changed || props.layout === "side") ? (
      <div className="flex items-center gap-1.5">
        {changed ? (
          <>
            <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-primary" />
            <span className="sr-only">{changed}</span>
          </>
        ) : null}
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
  const noteEl = <NoteLines lines={noteLines} />
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
          <div className={cn(stacked && "min-w-0 flex-1")}>{controlEl}</div>
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
    <div
      ref={box}
      data-form-layout="stacked"
      className={cn("flex flex-col", props.density === "compact" ? "gap-1" : "gap-2", fill && "min-h-0 flex-1")}
    >
      {header}
      <div className={cn("flex flex-col gap-1.5", fill && "flex-1")}>
        {row}
        {noteEl}
        {errorEl}
      </div>
    </div>
  )
}

/** A title over a group of fields: its words, then the fields, which a screen
 * reader hears as one group named by the title and described by its words. */
export function FormSection({
  title,
  help,
  notes,
  error,
  errorAnnounce = "silent",
  children,
}: {
  title: ReactElement | string
  help?: ReactNode
  notes?: readonly NoteLine[]
  error?: ReactNode
  errorAnnounce?: ErrorLineAnnounce
  // The fields, each a `FormGroup`; with none, the words stand alone.
  children?: ReactNode
}) {
  const base = useId()
  const titleId = `${base}title`
  const helpId = help != null ? `${base}help` : undefined
  const noteLines = drawnNotes(notes, base)
  const hasError = error != null && error !== false && error !== ""
  const errorId = hasError ? `${base}error` : undefined
  const fields = children != null && children !== false
  const errorEl = (
    <ErrorLine announce={errorAnnounce} size="xs" id={errorId}>
      {hasError ? error : null}
    </ErrorLine>
  )
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <span id={titleId} className={FIELD_LABEL_TEXT}>
          {title}
        </span>
        {helpId ? (
          <div id={helpId} className={FIELD_HELP_TEXT}>
            {help}
          </div>
        ) : null}
        <NoteLines lines={noteLines} />
        {fields ? null : errorEl}
      </div>
      {fields ? (
        <div
          role="group"
          aria-labelledby={titleId}
          aria-describedby={[helpId, ...noteLines.map((line) => line.id), errorId].filter(Boolean).join(" ") || undefined}
          className="flex flex-col gap-3"
        >
          {children}
          {errorEl}
        </div>
      ) : null}
    </div>
  )
}

// The note lines that have words, each with the id that describes a control by it.
function drawnNotes(notes: readonly NoteLine[] | undefined, base: string) {
  return (notes ?? []).flatMap((line, i) =>
    line.words == null || line.words === false || line.words === "" ? [] : [{ ...line, id: `${base}note${i}` }],
  )
}

function NoteLines({ lines }: { lines: ReturnType<typeof drawnNotes> }) {
  return lines.map((line) => (
    <div
      key={line.id}
      id={line.id}
      className={line.code ? FIELD_CODE_TEXT : line.tone === "warning" ? FIELD_WARNING_TEXT : FIELD_HELP_TEXT}
    >
      {line.words}
    </div>
  ))
}

// The parts of the wiring the control under `root` does not carry. It is found
// by its id (a single control) or by its name (a group), then each id of its
// description and its invalid mark must be on it.
function unwired(root: HTMLElement, field: FieldWiring): string[] {
  const attr = field["aria-label"] !== undefined ? "aria-label" : "aria-labelledby"
  const el =
    field.id !== undefined
      ? root.ownerDocument.getElementById(field.id)
      : Array.from(root.querySelectorAll(`[${attr}]`)).find((e) => e.getAttribute(attr) === field[attr])
  if (el === null || el === undefined || !root.contains(el)) return [field.id !== undefined ? "id" : attr]
  const missing: string[] = []
  const have = new Set((el.getAttribute("aria-describedby") ?? "").split(/\s+/))
  if (!(field["aria-describedby"] ?? "").split(/\s+/).filter(Boolean).every((id) => have.has(id))) {
    missing.push("aria-describedby")
  }
  if (field["aria-invalid"] !== undefined && el.getAttribute("aria-invalid") !== "true") missing.push("aria-invalid")
  return missing
}
