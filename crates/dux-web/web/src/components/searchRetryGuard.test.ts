// The search field and the retry control each exist once (`ui/search-field.tsx`,
// `ui/retry-button.tsx`) and every place that needs one uses it, so size, icon,
// touch floor, role and what Escape does cannot drift between copies. This
// guard reads source text and fails on the shapes a hand-built copy takes.
//
// It reads text, not the program. Comments are blanked first. It sees:
//  - a retry word that is a string literal on its own ("Retry", "Try again",
//    "... Retry"), a JSX text child, or a line of JSX text, however it is
//    wrapped (`{"Retry"}`, `<Button>Retry</Button>`, `label: "Retry"`);
//  - an input element (`input`, `Input`, `InputGroupInput`, `CommandInput`)
//    whose own opening tag names itself in a placeholder or aria-label as a
//    search, find or filter, or is `type="search"` or `role="searchbox"`;
//  - a `type: "search"` anywhere;
//  - an import of a search glyph from lucide-react, whatever its alias and
//    however the import is split over lines.
// It cannot see a field whose name is computed (`aria-label={label}`), one
// built from spread props, a retry whose word comes from a variable or a
// translation, or a copy written in a file outside `src`. A field that is
// named some other way and drawn with another glyph gets past it.
//
// Every exemption is one exact occurrence with a count, so an entry that no
// longer matches, or that matches more places than it was written for, fails.
import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { filesUnder, withoutComments } from "@/test/sourceFiles"

const srcDir = fileURLToPath(new URL("../", import.meta.url))

interface Hit {
  file: string
  rule: "retry" | "field" | "type-search" | "magnifier"
  text: string
}

const RETRY_WORD = /^(?:.*[\s.·])?Retry$|^Try again$/
// The opening tag of an input-like element, to its closing `>`, with braces
// and strings kept whole.
function inputTags(source: string): string[] {
  const tags: string[] = []
  const open = /<(?:input|Input|InputGroupInput|CommandInput)\b/g
  for (let m = open.exec(source); m; m = open.exec(source)) {
    let depth = 0
    let quote = ""
    let end = m.index
    for (; end < source.length; end++) {
      const c = source[end]
      if (quote) {
        if (c === "\\") end++
        else if (c === quote) quote = ""
      } else if (c === '"' || c === "'" || c === "`") quote = c
      else if (c === "{") depth++
      else if (c === "}") depth--
      else if (c === ">" && depth === 0) break
    }
    tags.push(source.slice(m.index, end + 1).replace(/\s+/g, " "))
  }
  return tags
}

const SEARCH_GLYPH = /^(?:Search(?:Icon|Code|Check|Slash|X)?|ScanSearch|FileSearch\d?|FolderSearch|Telescope)$/

function lucideNames(source: string): string[] {
  const names: string[] = []
  const imports = /import\s*(?:type\s*)?\{([^}]*)\}\s*from\s*["']lucide-react["']/g
  for (let m = imports.exec(source); m; m = imports.exec(source)) {
    for (const part of m[1].split(",")) {
      const name = part.trim().split(/\s+as\s+/)[0]
      if (SEARCH_GLYPH.test(name)) names.push(name)
    }
  }
  return names
}

function hitsIn(files: Record<string, string>): Hit[] {
  const out: Hit[] = []
  for (const [file, raw] of Object.entries(files)) {
    const source = withoutComments(raw)
    const literal = /(["'`])((?:\\.|(?!\1)[^\\\n])*)\1/g
    for (let m = literal.exec(source); m; m = literal.exec(source)) {
      if (RETRY_WORD.test(m[2].trim())) out.push({ file, rule: "retry", text: m[2].trim() })
    }
    const jsxText = />\s*(Retry|Try again)\s*</g
    for (let m = jsxText.exec(source); m; m = jsxText.exec(source)) out.push({ file, rule: "retry", text: m[1] })
    const ownLine = /^\s*(Retry|Try again)\s*$/gm
    for (let m = ownLine.exec(source); m; m = ownLine.exec(source)) out.push({ file, rule: "retry", text: m[1] })
    for (const tag of inputTags(source)) {
      const named = /\b(?:placeholder|aria-label)\s*=\s*\{?\s*["'`][^"'`]*\b(?:search|find|filter)/i.test(tag)
      const typed = /\btype\s*=\s*\{?\s*["']search["']|\brole\s*=\s*\{?\s*["']searchbox["']/.test(tag)
      if (named || typed) out.push({ file, rule: "field", text: tag })
    }
    if (/\btype\s*:\s*["']search["']/.test(source)) out.push({ file, rule: "type-search", text: "type: search" })
    for (const name of lucideNames(source)) out.push({ file, rule: "magnifier", text: name })
  }
  return out
}

// One entry per exact occurrence: the file, the rule, a piece of the matched
// text, and how many such occurrences there are.
const ALLOWED: { file: string; rule: Hit["rule"]; contains: string; count: number; reason: string }[] = [
  { file: "components/ui/retry-button.tsx", rule: "retry", contains: "Retry", count: 1, reason: "the shared control's own default word" },
  { file: "components/ui/search-field.tsx", rule: "magnifier", contains: "Search", count: 1, reason: "the shared field's magnifier" },
  { file: "components/ui/search-field.tsx", rule: "field", contains: 'role="searchbox"', count: 1, reason: "the shared field's own input" },
  { file: "components/modal/typeContract.tsx", rule: "type-search", contains: "type: search", count: 1, reason: "the compile-time contract's negative case: it proves a modal's field cannot be given type search" },
  { file: "components/ui/command.tsx", rule: "magnifier", contains: "SearchIcon", count: 1, reason: "cmdk's own input, which the macro popover's combobox is built on; it cannot render the shared field's input" },
  { file: "components/MacroPopover.tsx", rule: "field", contains: "Search macros", count: 1, reason: "the same cmdk input: a combobox whose role and keys cmdk owns, so it cannot be the shared field" },
  { file: "components/ConfigEditorDialog.tsx", rule: "retry", contains: "Retry", count: 1, reason: "a modal footer button, which the modal contract takes as data (a label and an action); there is no slot for the shared button" },
  { file: "components/LoginPage.tsx", rule: "retry", contains: "Try again", count: 3, reason: "the words the gate pages hand to the shared button through GateRetry; the sentences with Try again in them are longer strings and are not seen" },
  { file: "components/LoginPage.tsx", rule: "retry", contains: "Retry", count: 1, reason: "the word the unreachable page hands to the shared button through GateRetry" },
  { file: "components/FileTree.tsx", rule: "retry", contains: "Failed to load. Retry", count: 1, reason: "the tree row's own wording, handed to the shared button" },
  { file: "components/ChangedFiles.tsx", rule: "retry", contains: "Couldn't refresh · Retry", count: 1, reason: "the folder row's own wording, handed to the shared button" },
  { file: "components/ChangedFiles.tsx", rule: "magnifier", contains: "Search", count: 1, reason: "the glyph of the no-matching-files empty state, which is not a field" },
  { file: "components/RouteNotFoundScreen.tsx", rule: "magnifier", contains: "SearchX", count: 1, reason: "a not-found glyph, not a field" },
  { file: "components/AgentNotFound.tsx", rule: "magnifier", contains: "SearchX", count: 1, reason: "a not-found glyph, not a field" },
]

function judge(hits: Hit[]): { rest: Hit[]; wrongCount: string[] } {
  const seen = new Map<number, number>()
  const rest = hits.filter((h) => {
    const i = ALLOWED.findIndex((a) => a.file === h.file && a.rule === h.rule && h.text.includes(a.contains))
    if (i < 0) return true
    seen.set(i, (seen.get(i) ?? 0) + 1)
    return false
  })
  const wrongCount = ALLOWED.flatMap((a, i) =>
    (seen.get(i) ?? 0) === a.count ? [] : [`${a.file} (${a.rule}: ${a.contains}) is allowed ${a.count}, found ${seen.get(i) ?? 0}`],
  )
  return { rest, wrongCount }
}

describe("one search field and one retry control", () => {
  it("has no hand-built copy in any file under src", () => {
    const files = Object.fromEntries(
      filesUnder(srcDir)
        .filter((f) => /\.tsx?$/.test(f) && !/\.test\.|\.d\.ts$/.test(f))
        .map((f) => [relative(srcDir, f), readFileSync(f, "utf8")]),
    )
    const { rest, wrongCount } = judge(hitsIn(files))
    expect(
      rest.map((h) => `${h.file} (${h.rule}): ${h.text}`),
      "build it with SearchField (components/ui/search-field) or RetryButton / FailedLoad (components/ui/retry-button), or allow the exact occurrence with a reason",
    ).toEqual([])
    expect(wrongCount, "an allowed occurrence that is gone or has multiplied: change its entry").toEqual([])
  })

  it("sees the ways a copy can be spelled", () => {
    const cases: Record<string, string> = {
      "a/braced.tsx": `export const A = () => <Button>{"Retry"}</Button>`,
      "a/oneline.tsx": `export const B = () => <Button onClick={f}>Retry</Button>`,
      "a/prop.tsx": `export const C = { label: "Retry", tone: "primary" }`,
      "a/alone.tsx": `export const D = () => (\n  <Button>\n    <Icon />\n    Retry\n  </Button>\n)`,
      "a/phrase.tsx": `export const E = <B label='Failed to load. Retry' />`,
      "a/again.tsx": "export const F = `Try again`",
      "a/aliased.tsx": `import { Search as Magnifier } from "lucide-react"`,
      "a/other.tsx": `import {\n  Plus,\n  ScanSearch,\n} from "lucide-react"`,
      "a/typed.tsx": `export const G = () => <input type="search" value={v} />`,
      "a/named.tsx": `export const H = () => <Input\n  value={v}\n  onChange={(e) => set(e.target.value)}\n  aria-label="Find a thing"\n/>`,
      "a/bare.tsx": `export const I = () => <input placeholder={"Filter files…"} />`,
      "a/spread.tsx": `export const J = { type: "search" }`,
    }
    const hit = new Set(hitsIn(cases).map((h) => h.file))
    expect([...hit].sort()).toEqual(Object.keys(cases).sort())
  })

  it("does not take prose, comments or ordinary fields for a copy", () => {
    const quiet = {
      "ok.tsx": `// Retry the load, then Search again with type="search"
const msg = "That password did not work. Try again."
const a = <Input value={name} placeholder="Name" aria-label="Agent name" />
const b = <p>Retrying in a moment</p>
import { Plus, Trash2 } from "lucide-react"`,
    }
    expect(hitsIn(quiet)).toEqual([])
  })
})
