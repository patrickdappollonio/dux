import { readdirSync, readFileSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

// An error sentence in red is the `ErrorLine` component. This guard reads every
// source file under `src` (tests and the `ui` primitives excluded) and finds
// each bare `text-destructive` class in the code, comments dropped. Every one
// must be matched by one entry below, naming the file, the text of its line and
// the reason that applies to that occurrence. An occurrence with no entry fails,
// and so does an entry that no occurrence matches, so a hand-written red
// paragraph cannot slip in and a removed one cannot leave a stale exemption.
//
// What it cannot see, because it reads text and not the program:
// - a red sentence painted another way: a style attribute, a class built from
//   parts at runtime, `text-red-*`, or `text-destructive/90` and other
//   variants that carry a slash, which are seen as a different class;
// - a state variant such as `hover:text-destructive`, deliberately not counted;
// - the reasons themselves: that a line is standing, or already announced, is
//   a claim this test cannot check, only a reviewer can;
// - comment starts in unusual places: a regular expression that holds a quote
//   or `//`, or a `//` in unquoted JSX text such as a bare URL, can hide the
//   rest of its line; a template literal is read as one string, `${}` and all.

const root = fileURLToPath(new URL("../", import.meta.url))

function sourcesUnder(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name)
    if (e.isDirectory()) return e.name === "node_modules" || e.name === "ui" ? [] : sourcesUnder(p)
    return /\.tsx?$/.test(e.name) && !/\.test\.|\.d\.ts$/.test(e.name) ? [p] : []
  })
}

// The source with its comments blanked out and every line kept, so a comment
// start inside a string is not a comment and what follows it is still code.
// A quote ends at the end of its line, so a JSX apostrophe cannot swallow code.
export function withoutComments(source: string): string {
  let out = ""
  let i = 0
  let quote: string | null = null
  while (i < source.length) {
    const c = source[i]
    const next = source[i + 1]
    if (quote !== null) {
      out += c
      if (c === "\\" && next !== undefined && next !== "\n") {
        out += next
        i += 2
        continue
      }
      if (c === quote || (c === "\n" && quote !== "`")) quote = null
      i++
    } else if (c === "/" && next === "/") {
      while (i < source.length && source[i] !== "\n") i++
    } else if (c === "/" && next === "*") {
      const end = source.indexOf("*/", i + 2)
      const stop = end === -1 ? source.length : end + 2
      out += source.slice(i, stop).replace(/[^\n]/g, " ")
      i = stop
    } else {
      if (c === '"' || c === "'" || c === "`") quote = c
      out += c
      i++
    }
  }
  return out
}

const BARE = /(?<![:\w/-])text-destructive(?![\w/-])/

// The text of each line that holds a bare red class, one item per occurrence.
export function redLines(source: string): string[] {
  return withoutComments(source)
    .split("\n")
    .flatMap((line) => {
      const found = line.match(new RegExp(BARE.source, "g"))
      return found === null ? [] : found.map(() => line.trim())
    })
}

const STANDING =
  "a standing warning about a destructive act, there when the dialog opens: not an error, not announced"
const BRANCH_WARNING =
  "a warning drawn beside the branch checkbox it is about; not an error of a sent request, and not announced before this work either"

interface Allowed {
  file: string
  line: string
  reason: string
}
const ALLOWED: Allowed[] = [
  { file: "components/ErrorLine.tsx", line: "whitespace-pre-wrap text-destructive", reason: "the component's own log shape" },
  { file: "components/ErrorLine.tsx", line: "`${text} text-destructive`", reason: "the component's own sentence" },
  { file: "components/Notice.tsx", line: 'icon: "text-destructive"', reason: "the notice's icon colour" },
  { file: "components/Notice.tsx", line: 'heading: "text-destructive"', reason: "the notice's heading colour" },
  { file: "components/AttachedSection.tsx", line: "{renderProse(lead)}", reason: "inside the section's own alert region, which appears only after a refused press and is announced as a whole" },
  { file: "components/AttachedSection.tsx", line: "attachedDrivingLeadProse()", reason: "the standing list of devices already known to drive, shown before anything is sent: not announced" },
  { file: "components/ConfirmCloseEditorTabDialog.tsx", line: 'className="text-sm text-destructive"', reason: STANDING },
  { file: "components/ConfirmDeleteTerminalDialog.tsx", line: 'className="text-sm text-destructive"', reason: STANDING },
  { file: "components/ConfirmDiscardFileDialog.tsx", line: 'className="text-sm text-destructive"', reason: STANDING },
  { file: "components/ConfirmDiscardFilesDialog.tsx", line: 'className="text-sm text-destructive"', reason: STANDING },
  { file: "components/ConfirmReloadFileDialog.tsx", line: 'className="text-sm text-destructive"', reason: STANDING },
  { file: "components/ConfirmReopenEncodingDialog.tsx", line: 'className="text-sm text-destructive"', reason: STANDING },
  { file: "components/ConfirmVanishedEditorDialog.tsx", line: 'className="text-sm text-destructive"', reason: STANDING },
  { file: "components/SaveConflictDialog.tsx", line: 'className="text-sm text-destructive"', reason: `${STANDING} (the conflict is why the dialog opened)` },
  { file: "components/DeleteEntryDialog.tsx", line: 'className="text-sm text-destructive"', reason: "the line that holds the delete while a save is running, there when the dialog opens" },
  { file: "components/DeleteEntryDialog.tsx", line: 'className="text-sm text-destructive"', reason: STANDING },
  { file: "components/RenameEntryDialog.tsx", line: "{dirtyRefusal}", reason: "the dirty-file refusal, there when the dialog opens on a dirty file: standing, as DeleteEntryDialog's save line" },
  { file: "components/MoveEntryDialog.tsx", line: "{dirtyRefusal}", reason: "the dirty-file refusal, there when the dialog opens on a dirty file: standing, as DeleteEntryDialog's save line" },
  { file: "components/MoveEntryDialog.tsx", line: "{state.message}", reason: "a failed folder listing with its own Retry: left as it was while that row moves onto a shared failed-load component, whose announcement is settled when the pieces are combined" },
  { file: "components/DeleteSessionDialog.tsx", line: 'return <p className="text-sm text-destructive">', reason: BRANCH_WARNING },
  { file: "components/WorktreeFilesNote.tsx", line: '"text-destructive"', reason: `${STANDING} (what a delete would lose, once git has answered)` },
  { file: "components/WorktreesDialog.tsx", line: 'className="grid gap-2 text-sm text-destructive"', reason: `${STANDING} (the delete-worktree confirm)` },
  { file: "components/WorktreesDialog.tsx", line: 'className="grid gap-2 text-sm text-destructive"', reason: `${STANDING} (the delete-leftover-folder confirm)` },
  { file: "components/WorktreesDialog.tsx", line: 'className="text-sm text-destructive"', reason: BRANCH_WARNING },
  { file: "components/CustomizeWebappDialog.tsx", line: 'below && "text-destructive"', reason: "one word of the strength meter's label, not a sentence" },
  { file: "components/StandaloneAgentDialog.tsx", line: '"text-xs text-destructive"', reason: "the name field's help paragraph turning red, always present and tied to the field by aria-describedby" },
  { file: "components/StartupLogsDialog.tsx", line: 'className="text-destructive"', reason: "a span inside a danger Notice, which already announces it" },
  { file: "components/FileTree.tsx", line: "{rootState.message}", reason: "a failed-load row with its own Retry, built elsewhere" },
  { file: "components/ChangedFiles.tsx", line: "TriangleAlert", reason: "the icon of a failed-load row with its own Retry, built elsewhere" },
  { file: "components/EditorBody.tsx", line: "text-sm text-destructive", reason: "the editor pane's full-pane load-failure screen, not a dialog" },
  { file: "components/EditorBody.tsx", line: "text-sm text-destructive", reason: "the editor pane's second full-pane load-failure screen, not a dialog" },
  { file: "components/FlatAgentList.tsx", line: "text-destructive", reason: "a sidebar row's failure word, not a dialog" },
]

// Match each red line to one entry; what is left over on either side is wrong.
export function unmatched(found: { file: string; line: string }[], allowed: Allowed[]) {
  const free = [...allowed]
  const unexplained: string[] = []
  for (const hit of found) {
    const at = free.findIndex((a) => a.file === hit.file && hit.line.includes(a.line))
    if (at === -1) unexplained.push(`${hit.file}: ${hit.line}`)
    else free.splice(at, 1)
  }
  return { unexplained, stale: free.map((a) => `${a.file}: ${a.line}`) }
}

describe("no hand-written red error sentence", () => {
  it("keeps every red sentence in ErrorLine unless that occurrence is allowed with its own reason", () => {
    const found = sourcesUnder(root).flatMap((p) =>
      redLines(readFileSync(p, "utf8")).map((line) => ({ file: relative(root, p), line })),
    )
    const { unexplained, stale } = unmatched(found, ALLOWED)
    expect(unexplained, "show the error with <ErrorLine>, or allow this occurrence with its reason").toEqual([])
    expect(stale, "an allowed occurrence is gone: remove its entry").toEqual([])
  })

  it("sees a bare class and ignores variants and comments", () => {
    expect(redLines(`<p className="text-sm text-destructive">x</p>`)).toHaveLength(1)
    expect(redLines(`<p className={cn("a", ok && "text-destructive")} />`)).toHaveLength(1)
    expect(redLines(`<b className="hover:text-destructive not-focus:text-destructive/70 bg-destructive" />`)).toEqual([])
    expect(redLines(`// text-destructive\n/* text-destructive */`)).toEqual([])
  })

  it("does not lose code after a string that holds a comment start, nor count a trailing comment", () => {
    expect(redLines(`const a = "/*"\nconst b = "text-destructive"\n/* real */`)).toHaveLength(1)
    expect(redLines(`const a = "http://x" + "text-destructive"`)).toHaveLength(1)
    expect(redLines(`<p>don't</p>\n<p className="text-destructive">x</p>`)).toHaveLength(1)
    expect(redLines(`const a = 1 // text-destructive`)).toEqual([])
    expect(redLines(`const a = 1 /* text-destructive */ + 2`)).toEqual([])
  })

  it("holds one entry to one occurrence", () => {
    const a = { file: "f.tsx", line: "text-destructive", reason: "x" }
    const hit = { file: "f.tsx", line: '<p className="text-destructive">' }
    expect(unmatched([hit, hit], [a]).unexplained).toHaveLength(1)
    expect(unmatched([hit], [a, a]).stale).toHaveLength(1)
  })
})
