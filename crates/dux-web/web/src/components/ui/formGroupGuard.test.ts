// Every labelled field is drawn by `FormGroup` (a checkbox's by `CheckboxRow`),
// so its label and help look alike and are tied to the control. This guard
// reads the web UI's source and fails on a `<label>`, an `htmlFor`, a `span`, `p`,
// `div` or `legend` another element names in `aria-labelledby`, or one drawn in
// the label's classes written out, anywhere else, and on the shared looks used
// outside the two.
// It reads text, not the program, so it cannot see:
//
// - a label that is a component of its own (`<Title id={…}>`) rather than a `span`,
//   `p`, `div` or `legend`, or one whose id and `aria-labelledby` are written
//   differently (the match is on the expression's text);
// - words over a control tied to nothing at all, in a class string of their own
//   (the label look is only seen when its classes are the label's and no others);
// - a field named only by its placeholder or an `aria-label`;
// - a `<label>` built at run time (`createElement("label")`, `const Tag = "label"`);
// - test files, which it skips.
import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { sourceFilesUnder, withoutComments } from "@/test/sourceFiles"

const srcDir = fileURLToPath(new URL("../../", import.meta.url))

/** The files that draw a field's words themselves, each with its reason. */
const OWNERS: Record<string, string> = {
  "components/ui/form-group.tsx": "the form group",
  "components/CheckboxRow.tsx": "the checkbox row: the box on the left, its words on the right",
  "components/ui/form-text.ts": "where the shared looks are defined",
  "components/ui/search-field.tsx":
    "the search box is a label around its input, so a press anywhere in the box lands in it; it is named by its own `label` prop",
}

/** Files whose fields have not moved onto `FormGroup` yet: how many hand-written
 * labels each holds, and the step of the move that takes them. */
const STILL_TO_MOVE: Record<string, { labels: number; step: string }> = {
  "components/ChangeProviderDialog.tsx": { labels: 1, step: "3: dialog by dialog" },
  "components/CloneProjectDialog.tsx": { labels: 3, step: "3: dialog by dialog" },
  "components/LoginPage.tsx": { labels: 1, step: "3: dialog by dialog" },
}

/** Text that matches the label's shape and is not one: how many such elements each
 * file holds, and why none is a field's label. Pinned by count, like the files
 * still to move, so a new one in a listed file is looked at. */
const NOT_FIELDS: Record<string, { count: number; reason: string }> = {
  "components/ChangedFiles.tsx": { count: 1, reason: "the hidden words `Select <path>` that name a changed-file row's checkbox, which takes the row's leading slot and cannot be a `CheckboxRow`" },
  "components/ProjectList.tsx": { count: 1, reason: "the project's name in its row, which is also the name of the row's button; the row is a list item, not a field" },
  "components/CustomizeWebappDialog.tsx": { count: 1, reason: "the title of the browser-permission line under Desktop notifications: a sentence and a button named by its own words, no field" },
  "components/ChunkBoundary.tsx": { count: 1, reason: "the heading of the error card" },
  "components/FirstLoadDialog.tsx": { count: 1, reason: "the title of one step of the welcome list" },
  "components/MobileShell.tsx": { count: 1, reason: "the title of the Changes screen's header" },
  "components/NotRunningCard.tsx": { count: 1, reason: "the card's headline" },
  "components/StackNewDialog.tsx": { count: 1, reason: "the title of a row in the New dialog's list" },
  "components/terminal/partPieces.tsx": { count: 1, reason: "the headline of the file-drop overlay" },
}

const SHARED_LOOKS = /\b(?:FIELD_LABEL_TEXT|FIELD_HELP_TEXT|FIELD_WARNING_TEXT|FIELD_CODE_TEXT|CHECK_LABEL_TEXT)\b/

/** The whole opening tag that starts at `at` (`code[at]` is its `<`): it ends at the
 * first `>` outside a quote, a `{…}` expression and a template, so an arrow
 * function's `=>` does not end it. An unterminated tag reads to the end. */
export function tagAt(code: string, at: number): string {
  let depth = 0
  let quote: string | null = null
  for (let i = at; i < code.length; i++) {
    const c = code[i]
    if (quote !== null) {
      if (c === "\\") i++
      else if (c === quote) quote = null
    } else if (c === '"' || c === "'" || c === "`") quote = c
    else if (c === "{") depth++
    else if (c === "}") depth--
    else if (c === ">" && depth <= 0 && code[i - 1] !== "=") return code.slice(at, i + 1)
  }
  return code.slice(at)
}

/** The opening tags of the elements named in `names`, whole, with where each starts. */
function tags(code: string, names: string): { at: number; text: string }[] {
  const start = new RegExp(`<(?:${names})(?![\\w-])`, "g")
  return [...code.matchAll(start)].map((m) => ({ at: m.index, text: tagAt(code, m.index) }))
}

/** The text of the attribute `name` on `tag`, braces and quotes included, or null. */
function attr(tag: string, name: string): string | null {
  const m = new RegExp(`\\s${name}\\s*=\\s*`).exec(tag)
  return m === null ? null : valueAt(tag, m.index + m[0].length)
}

/** One attribute value starting at `from`: a quoted string or a balanced `{…}`. */
function valueAt(tag: string, from: number): string {
  if (tag[from] === '"' || tag[from] === "'") return tag.slice(from, tag.indexOf(tag[from], from + 1) + 1)
  let depth = 0
  let quote: string | null = null
  for (let i = from; i < tag.length; i++) {
    const c = tag[i]
    if (quote !== null) {
      if (c === "\\") i++
      else if (c === quote) quote = null
    } else if (c === '"' || c === "'" || c === "`") quote = c
    else if (c === "{") depth++
    else if (c === "}" && --depth === 0) return tag.slice(from, i + 1)
  }
  return tag.slice(from)
}

/** The shared label look written out: a class string made of nothing but the
 * classes `FIELD_LABEL_TEXT` is made of (and a `min-w-0` the group adds), with
 * the size and the weight both there. Any other class (a truncation, a margin,
 * a colour of its own) makes it a different piece of text. */
export function isLabelLook(classNames: string): boolean {
  const tokens = classNames.match(/[^\s"'`{}]+/g) ?? []
  if (tokens.some((t) => t.includes("${") || t === "cn(")) return false
  return (
    tokens.includes("text-sm") &&
    tokens.includes("font-medium") &&
    tokens.every((t) => ["text-sm", "font-medium", "text-foreground", "min-w-0"].includes(t))
  )
}

/** Each hand-written label in `code`: a `<label` tag, an `htmlFor` with no tag of
 * its own, a `span`, `p`, `div` or `legend` whose id another element names in
 * `aria-labelledby`, and an element of those drawn in the label look. */
export function handWrittenLabels(code: string): string[] {
  const labelTags = tags(code, "label")
  const loose = [...code.matchAll(/\bhtmlFor\s*[:=]/g)].filter(
    (m) => !labelTags.some((t) => t.at <= m.index && m.index < t.at + t.text.length),
  )
  const squash = (v: string) => v.replace(/\s+/g, "")
  const named = new Set(
    [...code.matchAll(/\baria-labelledby\s*=\s*/g)].map((m) => squash(valueAt(code, m.index + m[0].length))),
  )
  const words = tags(code, "span|p|div|legend").map((t) => t.text)
  const byId = words.filter((t) => {
    const id = attr(t, "id")
    return id !== null && named.has(squash(id))
  })
  const looks = words.filter((t) => {
    const cls = attr(t, "className")
    return cls !== null && isLabelLook(cls) && !byId.includes(t)
  })
  return [...labelTags.map((t) => t.text), ...loose.map((m) => m[0]), ...byId, ...looks]
}

function shipped(): string[] {
  return sourceFilesUnder(srcDir).map((f) => relative(srcDir, f))
}

function code(file: string): string {
  return withoutComments(readFileSync(`${srcDir}${file}`, "utf8"))
}

describe("every labelled field is a FormGroup (guard)", () => {
  it("finds no hand-written label outside the form group, the checkbox row, the files still to move and the listed non-fields", () => {
    const offenders: string[] = []
    const counts: Record<string, number> = {}
    const notFields: Record<string, { count: number; reason: string }> = {}
    for (const file of shipped()) {
      if (file in OWNERS) continue
      const found = handWrittenLabels(code(file))
      if (found.length === 0) continue
      if (file in NOT_FIELDS) notFields[file] = { count: found.length, reason: NOT_FIELDS[file].reason }
      else if (file in STILL_TO_MOVE) counts[file] = found.length
      else offenders.push(...found.map((tag) => `${file}: ${tag.replace(/\s+/g, " ").slice(0, 100)}`))
    }
    expect(offenders, "draw a labelled field with FormGroup").toEqual([])
    // A listed file pins its count: a new label there fails as one in a new
    // file does, and a file that moved is taken off the list.
    expect(counts).toEqual(Object.fromEntries(Object.entries(STILL_TO_MOVE).map(([f, e]) => [f, e.labels])))
    expect(notFields).toEqual(NOT_FIELDS)
  })

  it("keeps the shared label and help looks to the form group and the checkbox row", () => {
    const users = shipped()
      .filter((f) => SHARED_LOOKS.test(code(f)))
      .sort()
    expect(users).toEqual(["components/CheckboxRow.tsx", "components/ui/form-group.tsx", "components/ui/form-text.ts"])
  })

  it("sees each hand-written label that reading text can see", () => {
    const sees = [
      '<label className="text-sm font-medium">Provider</label>',
      '<label htmlFor="clone-address" className="text-sm font-medium">',
      "<label\n  htmlFor={id}\n  className={cn(a, b)}\n>",
      "<span id={labelId} />\n<Input id={id} />\n{createElement(\"label\", { htmlFor: id })}",
    ]
    for (const src of sees) expect(handWrittenLabels(src), src).toHaveLength(1)
    const more = [
      // an arrow in a handler does not end the tag, so the label is still one
      "<label onClick={() => open()} htmlFor={id} className={cn(a)}>Name</label>",
      // a span named by another element's aria-labelledby, however it is drawn
      '<span id={`${id}-label`} className="whatever">Mode</span>\n<div role="radiogroup" aria-labelledby={`${id}-label`} />',
      '<p id={titleId}>Mode</p><Choice aria-labelledby={titleId} />',
      // the label look written out by hand, in a few spellings
      '<p className="text-sm font-medium">Agent</p>',
      '<span className="font-medium text-sm text-foreground">Agent</span>',
      '<span className={"text-sm font-medium"}>Agent</span>',
    ]
    for (const src of more) expect(handWrittenLabels(src), src).toHaveLength(1)
    // a label with an arrow in it counts once, not twice
    expect(handWrittenLabels("<label onClick={() => a()} className={cn(b)} htmlFor={id}>")).toHaveLength(1)
    const passes = [
      '<FormGroup layout="stacked" control="single" label="Name">',
      "<DropdownMenuLabel>Agent</DropdownMenuLabel>",
      '<SearchField label="Search macros" />',
      "<labelled />",
      '<span id={nameId} className="truncate">Name</span><button aria-labelledby={other} />',
      '<span className="text-sm font-medium truncate">Name</span>',
      '<span className="text-sm text-muted-foreground">Name</span>',
      '<span className={cn("text-sm font-medium", dim && "opacity-50")}>Name</span>',
      '<button onClick={() => go()} className="text-sm font-medium">Go</button>',
      "// <label> in a comment is stripped before it is read",
    ]
    for (const src of passes) expect(handWrittenLabels(withoutComments(src)), src).toHaveLength(0)
  })
})
