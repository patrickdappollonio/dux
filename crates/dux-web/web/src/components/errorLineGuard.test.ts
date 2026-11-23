import { readdirSync, readFileSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

// An error sentence in red is the `ErrorLine` component. This guard reads every
// source file under `src` (tests and the `ui` primitives excluded) and counts
// the bare `text-destructive` class in each, after dropping comments. A count
// that is not in the table below fails, and so does a table entry that no
// longer matches, so a hand-written red paragraph cannot slip in and a removed
// one cannot leave a stale exemption.
//
// It reads text, not the program: a red sentence painted another way (a style
// attribute, a class built from parts at runtime, `text-red-*`) is not seen,
// and nor is a variant such as `hover:text-destructive`, which is a state.
// It does not tell an error from a standing warning; the table's reasons do.

const root = fileURLToPath(new URL("../", import.meta.url))

function sourcesUnder(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name)
    if (e.isDirectory()) return e.name === "node_modules" || e.name === "ui" ? [] : sourcesUnder(p)
    return /\.tsx?$/.test(e.name) && !/\.test\.|\.d\.ts$/.test(e.name) ? [p] : []
  })
}

const BARE = /(?<![:\w/-])text-destructive(?![\w/-])/g

function bareRedCount(source: string): number {
  const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "")
  return code.match(BARE)?.length ?? 0
}

const STANDING = "a standing warning about a destructive act, there when the dialog opens: not an error, not announced"
const ALLOWED: Record<string, { count: number; reason: string }> = {
  "components/ErrorLine.tsx": { count: 2, reason: "the component's own two shapes" },
  "components/Notice.tsx": { count: 2, reason: "the notice's heading and icon colour" },
  "components/AttachedSection.tsx": { count: 2, reason: `${STANDING} (the lead of each list of devices that would be cut off)` },
  "components/ConfirmCloseEditorTabDialog.tsx": { count: 1, reason: STANDING },
  "components/ConfirmDeleteTerminalDialog.tsx": { count: 1, reason: STANDING },
  "components/ConfirmDiscardFileDialog.tsx": { count: 1, reason: STANDING },
  "components/ConfirmDiscardFilesDialog.tsx": { count: 1, reason: STANDING },
  "components/ConfirmReloadFileDialog.tsx": { count: 1, reason: STANDING },
  "components/ConfirmReopenEncodingDialog.tsx": { count: 1, reason: STANDING },
  "components/ConfirmVanishedEditorDialog.tsx": { count: 1, reason: STANDING },
  "components/SaveConflictDialog.tsx": { count: 1, reason: `${STANDING} (the conflict is why the dialog opened)` },
  "components/DeleteEntryDialog.tsx": { count: 2, reason: `${STANDING}; the save-in-progress line is the reason the delete is held, there when it opens` },
  "components/DeleteSessionDialog.tsx": { count: 1, reason: `${STANDING} (the branch warning)` },
  "components/WorktreeFilesNote.tsx": { count: 1, reason: `${STANDING} (what a delete would lose, once git has answered)` },
  "components/WorktreesDialog.tsx": { count: 3, reason: `${STANDING} (the two delete confirms and the branch warning)` },
  "components/CustomizeWebappDialog.tsx": { count: 1, reason: "one word of the strength meter's label, not a sentence" },
  "components/StandaloneAgentDialog.tsx": { count: 1, reason: "the name field's help paragraph turning red, always present and tied to the field by aria-describedby" },
  "components/StartupLogsDialog.tsx": { count: 1, reason: "a span inside a danger Notice, which already announces it" },
  "components/FileTree.tsx": { count: 1, reason: "a failed-load row with its own Retry, built elsewhere" },
  "components/MoveEntryDialog.tsx": { count: 1, reason: "a failed-load row with its own Retry, built elsewhere" },
  "components/ChangedFiles.tsx": { count: 1, reason: "the icon of a failed-load row with its own Retry, built elsewhere" },
  "components/EditorBody.tsx": { count: 2, reason: "the editor pane's full-pane load-failure screens, not a dialog" },
  "components/FlatAgentList.tsx": { count: 1, reason: "a sidebar row's failure word, not a dialog" },
}

describe("no hand-written red error sentence", () => {
  it("keeps every red sentence in ErrorLine unless its exact count is allowed with a reason", () => {
    const found: Record<string, number> = {}
    for (const p of sourcesUnder(root)) {
      const n = bareRedCount(readFileSync(p, "utf8"))
      if (n > 0) found[relative(root, p)] = n
    }
    const wrong = Object.entries(found)
      .filter(([f, n]) => ALLOWED[f]?.count !== n)
      .map(([f, n]) => `${f}: ${n} (allowed ${ALLOWED[f]?.count ?? 0})`)
    expect(wrong, "show the error with <ErrorLine>, or allow the exact count with a reason").toEqual([])
    const stale = Object.keys(ALLOWED).filter((f) => !(f in found))
    expect(stale, "an allowed file no longer has red text: remove its entry").toEqual([])
  })

  it("sees a bare class and ignores variants and comments", () => {
    expect(bareRedCount(`<p className="text-sm text-destructive">x</p>`)).toBe(1)
    expect(bareRedCount(`<p className={cn("a", ok && "text-destructive")} />`)).toBe(1)
    expect(bareRedCount(`<b className="hover:text-destructive not-focus:text-destructive/70 bg-destructive" />`)).toBe(0)
    expect(bareRedCount(`// text-destructive\n/* text-destructive */`)).toBe(0)
  })
})
