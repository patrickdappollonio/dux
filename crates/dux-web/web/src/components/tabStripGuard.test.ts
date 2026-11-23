// Every tab strip in the web UI is built from `TabStrip` and `TabPill`, so the
// agent's provider tabs, an agent's terminal dock and the editor's file tabs
// look the same wherever they are. This guard reads the source and fails on a
// tab, a tab list or a strip container written anywhere else. That a `TabPill`
// sits in a `TabStrip` is not read here: the pill throws when it renders
// outside one.
//
// It reads text, not the program, so it cannot see: a role held in a variable
// or built at run time (`role={kind}`, `role={"t" + "ab"}`); a strip's classes
// joined from identifiers (`cn(SCROLLER, MUTED)`) or swapped for look-alikes
// (`bg-muted/25`, an inline style); a strip or pill drawn with no tab role and
// no strip classes at all; a re-export or wrapper of `TabPill` that is itself
// clean; files outside `src/`; and test files, which it skips.
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

/** Each occurrence that may stand, by file and its exact text, with its reason. */
const ALLOWED: Array<{ file: string; occurrence: string; reason: string }> = [
  { file: "components/TabStrip.tsx", occurrence: 'role="tablist"', reason: "the shared strip's list" },
  { file: "components/TabStrip.tsx", occurrence: 'role="tab"', reason: "the shared pill" },
  {
    file: "components/TabStrip.tsx",
    occurrence: "classes bg-muted/30 px-2 py-1.25",
    reason: "the shared strip's container",
  },
]

/** `src` without its comments, so a comment naming a role or a class is not a use. */
function withoutComments(src: string): string {
  return src.replace(
    /(["'`])(?:\\.|(?!\1)[^\\\n])*\1|\/\*[\s\S]*?\*\/|\/\/[^\n]*/g,
    (m) => (m.startsWith("/") ? " " : m),
  )
}

const ROLE_PATTERNS = [
  // A JSX attribute in any spelling: `role="tab"`, `role={"tab"}`,
  // `role={"tab" as const}`; never a selector such as `[role="tab"]`.
  /(?<![\w[-])role\s*=\s*(?:\{\s*)?(["'`])(?:tab|tablist)\1/g,
  // An object's key, for `createElement` or a spread: `{ role: "tab" }`.
  /(?<![\w-])(["']?)role\1\s*:\s*(["'`])(?:tab|tablist)\2/g,
  // The DOM's own way.
  /setAttribute\(\s*(["'`])role\1\s*,\s*(["'`])(?:tab|tablist)\2/g,
]

// The strip container's classes, in any order and split across the strings of
// one class expression.
const STRIP_CLASSES = ["bg-muted/30", "px-2", "py-1.25"]
const STRING = /(["'`])((?:\\.|(?!\1)[^\\])*)\1/g

const hasStripClasses = (strings: string[]) => {
  const tokens = strings.flatMap((s) => s.split(/\s+/))
  return STRIP_CLASSES.every((c) => tokens.includes(c))
}

/** How many class expressions, and strings outside one, carry the strip's classes. */
function stripContainers(code: string): number {
  let count = 0
  const spans: Array<[number, number]> = []
  // `className={…}`, `cn(…)`, `clsx(…)`, `cva(…)` and `twMerge(…)`, each with
  // everything inside its brackets.
  for (const m of code.matchAll(/\bclassName\s*=\s*\{|\b(?:cn|clsx|cva|twMerge)\s*\(/g)) {
    if (spans.some(([from, to]) => m.index >= from && m.index < to)) continue
    let depth = 1
    let at = m.index + m[0].length
    for (; at < code.length && depth > 0; at++) {
      const c = code[at]
      if (c === "{" || c === "(") depth++
      else if (c === "}" || c === ")") depth--
    }
    spans.push([m.index, at])
    const inside = [...code.slice(m.index, at).matchAll(STRING)].map((s) => s[2])
    if (hasStripClasses(inside)) count++
  }
  for (const m of code.matchAll(STRING)) {
    if (spans.some(([from, to]) => m.index >= from && m.index < to)) continue
    if (hasStripClasses([m[2]])) count++
  }
  return count
}

/** Every tab, tab list or strip container `src` writes, as its exact text. */
export function stripOccurrences(src: string): string[] {
  const code = withoutComments(src)
  const found: string[] = []
  for (const pattern of ROLE_PATTERNS) {
    for (const m of code.matchAll(pattern)) found.push(m[0])
  }
  for (let i = stripContainers(code); i > 0; i--) found.push(`classes ${STRIP_CLASSES.join(" ")}`)
  return found
}

const sources = walk(srcDir)
  .filter((path) => /\.tsx?$/.test(path) && !/\.test\.tsx?$/.test(path))
  .map((path) => ({ file: relative(srcDir, path), text: readFileSync(path, "utf8") }))

describe("tab strips are built from the shared TabStrip and TabPill (guard)", () => {
  it("writes a tab, a tab list or a strip container only where the allowlist says", () => {
    const unclaimed = ALLOWED.map((a) => ({ ...a }))
    const offenders: string[] = []
    for (const { file, text } of sources) {
      for (const occurrence of stripOccurrences(text)) {
        const at = unclaimed.findIndex((a) => a.file === file && a.occurrence === occurrence)
        if (at === -1) offenders.push(`${file}: ${occurrence}`)
        else unclaimed.splice(at, 1)
      }
    }
    expect(offenders, "build a tab strip from TabStrip and TabPill").toEqual([])
    expect(
      unclaimed.map((a) => `${a.file}: ${a.occurrence}`),
      "an allowlist entry that matches nothing is stale",
    ).toEqual([])
  })

  it("sees each way past it that reading text can see", () => {
    const sees = [
      '<div role="tab" />',
      "<div role='tab' />",
      '<div role={"tab"} />',
      '<div role={"tab" as const} />',
      "<div role={`tab`} />",
      '<div\n  role="tablist"\n/>',
      'createElement("div", { role: "tab" })',
      'const props = { "role": "tab", tabIndex: 0 }',
      'el.setAttribute("role", "tab")',
      '<div className="flex bg-muted/30 overflow-x-auto px-2 py-1.25" />',
      '<div className="py-1.25 px-2 bg-muted/30" />',
      '<div className={cn("flex px-2", active && "x", "bg-muted/30 py-1.25")} />',
      'const STRIP = "border-b bg-muted/30 px-2 py-1.25"',
    ]
    for (const src of sees) expect(stripOccurrences(src), src).not.toEqual([])
    const passes = [
      'el.closest(\'[role="tab"]\')',
      'screen.getAllByRole("tab")',
      '<div role="tabpanel" />',
      '<div role="button" data-tab="x" />',
      '// role="tab" in a comment',
      '<div className="bg-muted/30 px-2" />',
    ]
    for (const src of passes) expect(stripOccurrences(src), src).toEqual([])
    // Two strips in one file are two, even beside an allowed one.
    expect(
      stripOccurrences('<a className={cn("bg-muted/30 px-2 py-1.25")} /><b className="px-2 py-1.25 bg-muted/30" />'),
    ).toHaveLength(2)
  })
})
