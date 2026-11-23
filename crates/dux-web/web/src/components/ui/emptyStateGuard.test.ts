import { readdirSync, readFileSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { classGroupsOf } from "@/test/classGroups"

// A surface with nothing to show says so through the shared `EmptyState`, which
// decides its padding, type and colour. This guard reads every source file under
// `src` (all folders, .ts and .tsx, tests excluded) and fails on the two looks a
// hand-built one takes:
//
// - "quiet line": muted text, centred, with vertical room of a line or more
//   around it (`py-3` and up, `p-3` and up, `my-auto`, or a fixed `h-64`);
// - "filled pane": muted text centred both ways inside a box that fills what it
//   is in (`h-full`, `flex-1`, `min-h-*`, `size-full`, or a fixed height).
//
// Class text is grouped per `className` string, per `className={...}`
// expression and per `cn(` / `clsx(` / `cva(` / `twMerge(` call, so a look split
// across the pieces of one call is seen, and any text size, and any strength of
// the muted colour, counts. It reads text, not the program: it cannot see a look
// whose halves sit in separate variables or statements, one built at runtime, one
// set from a stylesheet or a style attribute, or an empty state drawn without
// muted text or without centring. Lines that wait or failed are not empty states
// and are allowed below by exact occurrence; so is anything else with the look.

function sourcesUnder(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name)
    if (e.isDirectory()) return e.name === "node_modules" ? [] : sourcesUnder(p)
    return /\.tsx?$/.test(e.name) && !/\.test\.|\.d\.ts$/.test(e.name) ? [p] : []
  })
}

const root = fileURLToPath(new URL("../../", import.meta.url))

const token = (re: string) => new RegExp(`(?:^|\\s)${re}(?=\\s|$)`)
const MUTED = token("text-muted-foreground(?:/\\d+)?")
const CENTERED = token("text-center")
const ROOMY = token("(?:py|p|my)-(?:[3-9]|\\d{2,})|my-auto|h-(?:[4-9]\\d|\\d{3,})|h-\\[[^\\]]+\\]")
const BOTH_WAYS = (t: string) => token("items-center").test(t) && token("justify-center").test(t)
const FILLS = token("(?:h-full|size-full|flex-1|min-h-\\S+|h-(?:[4-9]\\d|\\d{3,}))")

type Look = "quiet-line" | "filled-pane"

const LOOKS: Record<Look, (text: string) => boolean> = {
  "quiet-line": (t) => MUTED.test(t) && CENTERED.test(t) && ROOMY.test(t),
  "filled-pane": (t) => MUTED.test(t) && BOTH_WAYS(t) && FILLS.test(t),
}

interface Finding {
  file: string
  look: Look
  text: string
}

function findHandBuiltEmptyStates(files: Record<string, string>): Finding[] {
  const out: Finding[] = []
  for (const [file, source] of Object.entries(files)) {
    for (const text of classGroupsOf(source)) {
      for (const look of Object.keys(LOOKS) as Look[]) {
        if (LOOKS[look](text)) out.push({ file, look, text })
      }
    }
  }
  return out
}

// One entry per exact occurrence: the file, the look, a piece of the grouped
// class text that identifies it and how many times it occurs. An entry that
// matches fewer times than `count` fails the suite, and one more occurrence than
// `count` is a finding.
const ALLOWED: { file: string; look: Look; contains: string; count: number; reason: string }[] = [
  { file: "components/ui/empty.tsx", look: "quiet-line", contains: "px-3 py-6 text-center text-sm text-balance text-muted-foreground", count: 1, reason: "the component's own compact line" },
  { file: "components/ConfigEditorDialog.tsx", look: "quiet-line", contains: "py-12 text-center text-sm text-muted-foreground", count: 1, reason: "config.toml while it loads: a loading line" },
  { file: "components/ConfigEditorDialog.tsx", look: "filled-pane", contains: "flex h-full items-center justify-center text-muted-foreground", count: 1, reason: "a spinner while the editor loads: a loading state" },
  { file: "components/MacrosDialog.tsx", look: "quiet-line", contains: "py-10 text-center text-sm text-muted-foreground", count: 1, reason: "the macros while they load: a loading line" },
  { file: "components/PullRequestsDialog.tsx", look: "quiet-line", contains: "py-6 text-center text-sm text-muted-foreground", count: 1, reason: "the branches tab while it reads them, or why it could not: a loading or failed line" },
  { file: "components/EditorBody.tsx", look: "filled-pane", contains: "flex h-full items-center justify-center text-muted-foreground", count: 2, reason: "spinners while a buffer or the diff loads: loading states" },
  { file: "components/EditorBody.tsx", look: "filled-pane", contains: "flex h-full items-center justify-center px-4 text-center text-sm text-muted-foreground", count: 1, reason: "CenteredPane: why a file cannot be shown (binary, too large, no diff), a message about a file that exists and not an empty list" },
  { file: "components/StandaloneEditor.tsx", look: "filled-pane", contains: "flex h-full items-center justify-center text-muted-foreground", count: 1, reason: "a spinner while the tab boots: a loading state" },
  { file: "components/ui/avatar.tsx", look: "filled-pane", contains: "flex size-full items-center justify-center rounded-full bg-muted", count: 1, reason: "the avatar's initials fallback, not an empty state" },
]

function unallowed(findings: Finding[]): { rest: Finding[]; unused: string[] } {
  const seen = ALLOWED.map(() => 0)
  const rest = findings.filter((f) => {
    const i = ALLOWED.findIndex(
      (a, k) =>
        a.file === f.file && a.look === f.look && f.text.includes(a.contains) && seen[k] < a.count,
    )
    if (i < 0) return true
    seen[i]++
    return false
  })
  const unused = ALLOWED.filter((a, i) => seen[i] < a.count).map((a) => `${a.file}: ${a.contains}`)
  return { rest, unused }
}

describe("empty states go through the shared EmptyState", () => {
  it("finds no hand-built empty state in any folder under src", () => {
    const files = Object.fromEntries(
      sourcesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
    )
    const { rest, unused } = unallowed(findHandBuiltEmptyStates(files))
    expect(
      rest.map((f) => `${f.file} (${f.look}): ${f.text}`),
      "use EmptyState from components/ui/empty, or allow the exact occurrence with a reason",
    ).toEqual([])
    expect(unused, "an allowed occurrence that no longer exists: remove its entry").toEqual([])
  })

  it("sees the ways the look can be written or split", () => {
    const cases: Record<string, string> = {
      "a/one.tsx": `const A = () => <p className="py-2 text-xs text-muted-foreground/80 text-center py-4" />`,
      "a/b/two.tsx": `const B = () => <div className={cn("text-center", "text-muted-foreground", cond && "py-8")} />`,
      "three.tsx": `const C = () => <div className="flex h-full items-center justify-center text-sm text-muted-foreground" />`,
      "four.tsx": "const D = `p-6 text-center text-muted-foreground/70 ${size}`",
    }
    const hit = new Set(findHandBuiltEmptyStates(cases).map((f) => f.file))
    expect([...hit].sort()).toEqual(Object.keys(cases).sort())
  })

  it("does not take unrelated classes for an empty state", () => {
    const quiet = {
      "ok.tsx": `const a = <p className="text-center text-muted-foreground py-1" />
const b = <div className="flex h-full items-center justify-center bg-card" />
const c = <><p className="py-6 text-center" /><span className="text-muted-foreground" /></>
const d = <p className="hover:text-muted-foreground md:py-6 text-center" />`,
    }
    expect(findHandBuiltEmptyStates(quiet)).toEqual([])
  })
})
