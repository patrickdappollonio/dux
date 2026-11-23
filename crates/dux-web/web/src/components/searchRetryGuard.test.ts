// The search field and the retry control each exist once (`ui/search-field.tsx`,
// `ui/retry-button.tsx`) and every place that needs one uses it, so size, icon,
// touch floor and what Escape does cannot drift between copies. This guard
// reads source and fails on the shapes a hand-built copy takes.
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const srcDir = fileURLToPath(new URL("../", import.meta.url))

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

const files = walk(srcDir)
  .filter((f) => /\.tsx$/.test(f) && !f.includes(".test."))
  .map((f) => ({ name: relative(srcDir, f), source: readFileSync(f, "utf8") }))

// The files that are the shared components, and what is not a copy of them.
const SEARCH_ALLOWED: Record<string, string> = {
  "components/ui/search-field.tsx": "the shared field",
  "components/ui/command.tsx":
    "cmdk's own input, which the macro popover's combobox is built on; it cannot render the shared field's input",
}
const RETRY_ALLOWED: Record<string, string> = {
  "components/ui/retry-button.tsx": "the shared control",
  "components/ConfigEditorDialog.tsx":
    "the dialog's footer button, which the modal contract takes as data (a label and an action)",
  "components/OfflineOverlay.tsx":
    "Reconnect on the offline cover is the app socket's own act, with its own budget and countdown",
  "components/terminal/partPieces.tsx":
    "Reconnect on a terminal's cover re-attaches that terminal, which is not a repeated load",
}

describe("one search field", () => {
  it("no file draws a magnifier beside a text input of its own", () => {
    const offenders = files
      .filter(({ name }) => !(name in SEARCH_ALLOWED))
      .filter(({ source }) => /\bSearch(?:Icon)?\b[^;\n]*from "lucide-react"|^\s+Search(?:Icon)?,$/m.test(source))
      .filter(({ source }) => /<(?:input|Input|InputGroup|CommandInput)\b/.test(source))
      .map(({ name }) => name)
    expect(offenders, "build it with SearchField from components/ui/search-field").toEqual([])
  })

  it("no field is type=search", () => {
    const offenders = files.filter(({ source }) => /type=["']search["']/.test(source)).map(({ name }) => name)
    expect(offenders, "type=search clears itself on Escape and draws a clear button of its own").toEqual([])
  })
})

describe("one retry control", () => {
  it("no file spells a Retry button of its own", () => {
    const offenders = files
      .filter(({ name }) => !(name in RETRY_ALLOWED))
      .filter(({ source }) => /^\s*Retry\s*$/m.test(source) || /Failed to load\. Retry/.test(source))
      .map(({ name }) => name)
    expect(offenders, "use RetryButton or FailedLoad from components/ui/retry-button").toEqual([])
  })
})
