// Every labelled field is drawn by `FormGroup` (a checkbox's by `CheckboxRow`),
// so its label and help look alike and are tied to the control. This guard
// reads the web UI's source and fails on a `<label>` or an `htmlFor` written
// anywhere else, and on the shared label and help looks used outside the two.
// It reads text, not the program, so it cannot see:
//
// - a label drawn as a `span` or `p` and tied by `aria-labelledby` (how a group
//   of controls is named), or words over a control tied to nothing at all;
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
  "components/CustomizeWebappDialog.tsx": { labels: 2, step: "2: the Preferences row" },
  "components/ChangeProviderDialog.tsx": { labels: 1, step: "3: dialog by dialog" },
  "components/CloneProjectDialog.tsx": { labels: 2, step: "3: dialog by dialog" },
  "components/LoginPage.tsx": { labels: 1, step: "3: dialog by dialog" },
}

const SHARED_LOOKS = /\b(?:FIELD_LABEL_TEXT|FIELD_HELP_TEXT|CHECK_LABEL_TEXT)\b/

/** Each hand-written label in `code`: a `<label` tag, or an `htmlFor` with no tag of its own. */
export function handWrittenLabels(code: string): string[] {
  const tags = [...code.matchAll(/<label(?![\w-])[^>]*/g)]
  const inTag = (at: number) => tags.some((t) => t.index <= at && at < t.index + t[0].length)
  const loose = [...code.matchAll(/\bhtmlFor\s*[:=]/g)].filter((m) => !inTag(m.index))
  return [...tags, ...loose].map((m) => m[0])
}

function shipped(): string[] {
  return sourceFilesUnder(srcDir).map((f) => relative(srcDir, f))
}

function code(file: string): string {
  return withoutComments(readFileSync(`${srcDir}${file}`, "utf8"))
}

describe("every labelled field is a FormGroup (guard)", () => {
  it("finds no hand-written label outside the form group, the checkbox row and the files still to move", () => {
    const offenders: string[] = []
    const counts: Record<string, number> = {}
    for (const file of shipped()) {
      if (file in OWNERS) continue
      const found = handWrittenLabels(code(file))
      if (found.length === 0) continue
      if (file in STILL_TO_MOVE) counts[file] = found.length
      else offenders.push(...found.map((tag) => `${file}: ${tag.replace(/\s+/g, " ").slice(0, 100)}`))
    }
    expect(offenders, "draw a labelled field with FormGroup").toEqual([])
    // A listed file pins its count: a new label there fails as one in a new
    // file does, and a file that moved is taken off the list.
    expect(counts).toEqual(Object.fromEntries(Object.entries(STILL_TO_MOVE).map(([f, e]) => [f, e.labels])))
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
    const passes = [
      '<FormGroup layout="stacked" control="single" label="Name">',
      "<DropdownMenuLabel>Agent</DropdownMenuLabel>",
      '<SearchField label="Search macros" />',
      "<labelled />",
      "// <label> in a comment is stripped before it is read",
    ]
    for (const src of passes) expect(handWrittenLabels(withoutComments(src)), src).toHaveLength(0)
  })
})
