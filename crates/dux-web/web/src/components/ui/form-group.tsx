import { Fragment, useEffect, useId, useRef, type ReactElement, type ReactNode } from "react"

import { ErrorLine, type ErrorLineAnnounce } from "@/components/ErrorLine"
import { MatchHighlight } from "@/components/MatchHighlight"
import {
  FIELD_CODE_TEXT,
  FIELD_HELP_TEXT,
  FIELD_LABEL_TEXT,
  FIELD_OPTIONAL_TEXT,
  FIELD_WARNING_TEXT,
} from "@/components/ui/form-text"
import { useIsMobile } from "@/hooks/use-mobile"
import { matchCharRanges } from "@/lib/searchMatch"
import type { SettingDescriptor } from "@/lib/settingsDescriptors"
import { cn } from "@/lib/utils"

/** What the control spreads onto itself, so it is named and described by the group. */
export interface FieldWiring {
  id?: string
  "aria-label"?: string
  "aria-labelledby"?: string
  "aria-describedby"?: string
  "aria-invalid"?: true
}

/** The wiring of a control its words name but a press on them leaves alone.
 * An `<input>` or a `<textarea>` refuses it, and so does a wrapper typed `TextBoxWiring`;
 * a wrapper typed `FieldWiring` takes it, and only the development check sees a text box there. */
export interface NamedWiring extends FieldWiring {
  capture?: "a named control is not a text box"
  rows?: "a named control is not a text box"
}

/** What a wrapper around a text box takes: `FieldWiring` that a "named" field's does not fit. */
export type TextBoxWiring = FieldWiring & { rows?: number }

// "single" is one control a `<label>` points at, so a press on the words
// reaches it; "named" is one control named by the words' id, which a press
// leaves alone; "group" is a set of controls (a radio group) named by that id.
export type FieldControl = "single" | "named" | "group"
type WiringFor<C extends FieldControl> = C extends "named" ? NamedWiring : FieldWiring

// A field is named by words drawn above or beside it, or, with none drawn, by a
// name only a screen reader says. Neither may be missing: a label is a string or
// an element, so a value that may be undefined, null or false does not compile.
// The compiler cannot see an empty string, so the group reports one when drawn.
// `optional` says after the label, and in the name, that the field may be left empty.
// `helpAt` says where a bare field's help sits, "below" its control (the default) or
// "above" it; a labelled field's help always sits between its label and its control.
type FieldName =
  | { label: ReactElement | string; optional?: true; spokenName?: never; helpAt?: never }
  | { spokenName: string; label?: never; optional?: never; helpAt?: "above" | "below" }

// One line about the current value. A "warning" one reads as the warning it is;
// a `setting` one is that setting's config key in monospace, `lit` marking the
// searched words in it. Only a setting's key is drawn so, never a name.
export type NoteLine =
  | { words: ReactNode; tone?: "warning"; setting?: never; lit?: never }
  | { setting: SettingDescriptor; lit?: readonly string[]; words?: never; tone?: never }

// The layout decides where the note lines and the error go, never the width of
// the screen. "side" keeps all of a field's words (label, help, notes, error)
// in one block beside the control, and on a phone a "wide" control goes under
// that whole block, so the notes are above it. "stacked" draws the label and
// help, then the control, then the notes and the error under it (the help too
// when no label is drawn to sit with); "fill" makes
// the group and its control take the height left in a column, and "compact"
// sets the label 4px above its control instead of 8px, for the fields of a `FormSection`.
type FieldLayout =
  | { layout: "side"; width?: "compact" | "wide"; height?: never; density?: never; helpAt?: never }
  | { layout: "stacked"; height?: "content" | "fill"; density?: "regular" | "compact"; width?: never }

export type FormGroupProps<C extends FieldControl = FieldControl> = FieldName &
  FieldLayout & {
    control: C
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
    children: (field: WiringFor<C>) => ReactNode
  }

/** One labelled field: its words, its control and how the two are tied, in one of two layouts. */
export function FormGroup<C extends FieldControl>(props: FormGroupProps<C>) {
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

  const field: WiringFor<C> = {
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
    if (control === "named" && missing.length === 0 && namedTextBox(box.current, field)) {
      console.error("FormGroup: a text box is named by its words alone; make it a \"single\" control, so a press on its label reaches it.")
    }
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
      {props.optional ? <> <span className={FIELD_OPTIONAL_TEXT}>(optional)</span></> : null}
    </label>
  ) : (
    <span id={labelId} onClick={control === "group" ? focusGroup : undefined} className={cn(FIELD_LABEL_TEXT, "min-w-0")}>
      {props.label}
      {props.optional ? <> <span className={FIELD_OPTIONAL_TEXT}>(optional)</span></> : null}
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
  // A stacked field's error is the size of a dialog's sentences, a side or compact one's that of its help.
  const errorSize = props.layout === "stacked" && props.density !== "compact" ? "sm" : "xs"
  const errorEl = (
    <ErrorLine announce={errorAnnounce} size={errorSize} id={errorId}>
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
      labelLine
    )
  // Always drawn so the control keeps its place in the tree; with nothing on it and no
  // height to take it is `display: contents`, which makes no box to change the layout.
  const rowed = before != null || after != null || fill
  const row = (
    <div
      data-form-control={rowed ? "" : undefined}
      className={rowed ? cn("flex min-w-0 gap-2", fill ? "flex-1 items-stretch" : "items-center") : "contents"}
    >
      {before}
      {controlEl}
      {after}
    </div>
  )
  return (
    <div
      ref={box}
      data-form-layout="stacked"
      className={cn("flex flex-col", props.density === "compact" ? "gap-1" : "gap-2", fill && "min-h-0 flex-1")}
    >
      {header}
      <div className={cn("flex flex-col gap-1.5", fill && "flex-1")}>
        {labelLine === null && props.helpAt === "above" ? helpEl : null}
        {row}
        {labelLine === null && props.helpAt !== "above" ? helpEl : null}
        {noteEl}
        {errorEl}
      </div>
    </div>
  )
}

/** A line of a dialog that also describes a field (its `id` in the field's `describedBy`),
 * in the size and colour of the field's own help. */
export function HelpLine({ id, children }: { id: string; children: ReactNode }) {
  return (
    <p id={id} className={FIELD_HELP_TEXT}>
      {children}
    </p>
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
  const box = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!import.meta.env.DEV || box.current === null) return
    if (!box.current.ownerDocument.getElementById(titleId)?.textContent?.trim()) {
      console.error("FormSection: the section has no title; give it a title that is not empty.")
    }
    const group = box.current.querySelector('[role="group"]')
    if (group !== null && group.querySelector("[data-form-layout]") === null) {
      console.error("FormSection: the group holds no field; draw each field with FormGroup, or leave the children out.")
    }
  })
  const errorEl = (
    <ErrorLine announce={errorAnnounce} size="xs" id={errorId}>
      {hasError ? error : null}
    </ErrorLine>
  )
  return (
    <div ref={box} className="flex flex-col gap-3">
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
    line.setting === undefined && (line.words == null || line.words === false || line.words === "")
      ? []
      : [{ ...line, id: `${base}note${i}` }],
  )
}

function NoteLines({ lines }: { lines: ReturnType<typeof drawnNotes> }) {
  return lines.map((line) => (
    <div
      key={line.id}
      id={line.id}
      className={line.setting ? FIELD_CODE_TEXT : line.tone === "warning" ? FIELD_WARNING_TEXT : FIELD_HELP_TEXT}
    >
      {line.setting ? <ConfigKey text={line.setting.key} lit={line.lit ?? []} /> : line.words}
    </div>
  ))
}

// A config key wraps after a dot or an underscore, never inside a word.
function ConfigKey({ text, lit }: { text: string; lit: readonly string[] }) {
  const ranges = matchCharRanges(text, lit)
  const pieces = text.match(/[^._]+[._]?|[._]/g) ?? [""]
  let offset = 0
  return pieces.map((piece, i) => {
    const at = offset
    offset += Array.from(piece).length
    const own = ranges
      .map((r) => ({ start: r.start - at, end: r.end - at }))
      .filter((r) => r.end > 0)
      .map((r) => ({ start: Math.max(0, r.start), end: r.end }))
    return (
      <Fragment key={at}>
        {i > 0 ? <wbr /> : null}
        <MatchHighlight text={piece} ranges={own} />
      </Fragment>
    )
  })
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
  if (
    (el.getAttribute("aria-label") ?? undefined) !== field["aria-label"] ||
    (el.getAttribute("aria-labelledby") ?? undefined) !== field["aria-labelledby"]
  ) {
    missing.push("its name")
  }
  return missing
}

// Whether the control a "named" field wired is a box text is typed into.
function namedTextBox(root: HTMLElement, field: FieldWiring): boolean {
  const attr = field["aria-label"] !== undefined ? "aria-label" : "aria-labelledby"
  const el = Array.from(root.querySelectorAll(`[${attr}]`)).find((e) => e.getAttribute(attr) === field[attr])
  return el?.matches("textarea, input:not([type=checkbox], [type=radio], [type=range], [type=color])") ?? false
}
