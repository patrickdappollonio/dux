import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { classGroupsOf } from "@/test/classGroups"
import { sourceFilesUnder } from "@/test/sourceFiles"

// A warning, danger, info or neutral notice is the `Notice` component. This
// guard reads every source file under `src` (all folders, .ts and .tsx, tests
// excluded) and fails on classes that paint a notice's frame by hand.
//
// It reads text, not the program. Class strings are grouped by where they sit:
// one `className="..."` string, one `className={...}` expression, or one
// `cn(` / `clsx(` / `cva(` / `twMerge(` call with everything inside it, nested
// objects and arrays included; any other string literal is a group of its own.
// So a frame split across the strings of one call is seen. It cannot see a
// frame whose halves live in separate variables or statements, one built from
// parts at runtime, a class set through a style attribute or a stylesheet, or a
// notice drawn without these classes at all. Prefixed variants (`hover:`,
// `aria-invalid:`) are never counted, since they are states and not a frame.

const root = fileURLToPath(new URL("../", import.meta.url))

type Look = "tinted-border" | "info-frame" | "neutral-frame"

const TOKEN = (re: string) => new RegExp(`(?:^|\\s)${re}(?=\\s|$)`)
const LOOKS: Record<Look, (text: string) => boolean> = {
  // Any unprefixed warning or danger border: a notice frame, with or without
  // its tint.
  "tinted-border": (t) => TOKEN("border-(?:dux-warning|destructive)(?:/\\d+)?").test(t),
  "info-frame": (t) => TOKEN("border-border").test(t) && TOKEN("bg-muted/40").test(t),
  "neutral-frame": (t) => TOKEN("border-border").test(t) && TOKEN("bg-card").test(t),
}

interface Finding {
  file: string
  look: Look
  text: string
}

function findHandBuiltNotices(files: Record<string, string>): Finding[] {
  const out: Finding[] = []
  for (const [file, source] of Object.entries(files)) {
    for (const text of classGroupsOf(source)) {
      for (const [look, test] of Object.entries(LOOKS) as [Look, (t: string) => boolean][]) {
        if (test(text)) out.push({ file, look, text })
      }
    }
  }
  return out
}

// One entry per exact occurrence: the file, the look, and a piece of the
// grouped class text that identifies it. An entry that matches nothing fails.
const ALLOWED: { file: string; look: Look; contains: string; reason: string }[] = [
  { file: "components/Notice.tsx", look: "tinted-border", contains: "border-dux-warning/40 bg-dux-warning/10", reason: "the component's warning frame" },
  { file: "components/Notice.tsx", look: "tinted-border", contains: "border-destructive/50 bg-destructive/10", reason: "the component's danger frame" },
  { file: "components/Notice.tsx", look: "info-frame", contains: "border-border bg-muted/40 text-muted-foreground", reason: "the component's info frame" },
  { file: "components/Notice.tsx", look: "neutral-frame", contains: "border-border bg-card text-card-foreground", reason: "the component's neutral frame" },
  { file: "components/DeleteSessionDialog.tsx", look: "tinted-border", contains: "border-dux-warning/30 bg-dux-warning/10", reason: "a branch-check pill: a rounded-full status chip in a row, not a sentence-and-buttons notice" },
  { file: "components/DiffViewer.tsx", look: "info-frame", contains: "border-b border-border bg-muted/40 px-3 py-2 text-xs", reason: "the diff's quiet 12px caption strip, kept exactly as it was and not a notice" },
  { file: "components/DiffHeadViewer.tsx", look: "info-frame", contains: "border-b border-border bg-muted/40 px-3 py-2 text-xs", reason: "the diff's quiet 12px caption strip, kept exactly as it was and not a notice" },
  { file: "components/LoginPage.tsx", look: "info-frame", contains: "rounded-lg border border-border bg-muted/40 p-3 font-mono", reason: "a preformatted code block, not a sentence" },
]

function unallowed(findings: Finding[]): { rest: Finding[]; unused: string[] } {
  const used = new Set<number>()
  const rest = findings.filter((f) => {
    const i = ALLOWED.findIndex((a) => a.file === f.file && a.look === f.look && f.text.includes(a.contains))
    if (i < 0) return true
    used.add(i)
    return false
  })
  const unused = ALLOWED.filter((_, i) => !used.has(i)).map((a) => `${a.file}: ${a.contains}`)
  return { rest, unused }
}

describe("no hand-built notice frame", () => {
  it("keeps every notice frame inside Notice, in every folder under src", () => {
    const files = Object.fromEntries(
      sourceFilesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
    )
    const { rest, unused } = unallowed(findHandBuiltNotices(files))
    expect(
      rest.map((f) => `${f.file} (${f.look}): ${f.text}`),
      "build the notice with <Notice>, or allow the exact occurrence with a reason",
    ).toEqual([])
    expect(unused, "an allowed occurrence that no longer exists: remove its entry").toEqual([])
  })

  it("sees the ways a frame can be split or hidden", () => {
    const cases: Record<string, string> = {
      "a/one.tsx": `export const A = () => <div className="rounded-md border border-dux-warning/40 bg-dux-warning/10 p-3" />`,
      "a/b/two.tsx": `export const B = () => <div className={cn("border-destructive/50", "p-3", cond && "bg-destructive/10")} />`,
      "three.tsx": `const X = clsx("flex", { "border-dux-warning": on })`,
      "four.tsx": `const Y = \`border-border bg-muted/40 \${size}\``,
      "five.tsx": `const Z = cn("border-border", cond ? "bg-card" : "")`,
    }
    const hit = new Set(findHandBuiltNotices(cases).map((f) => f.file))
    expect([...hit].sort()).toEqual(Object.keys(cases).sort())
  })

  it("does not take a state variant or two unrelated elements for a frame", () => {
    const quiet = {
      "ok.tsx": `const a = <input className="aria-invalid:border-destructive hover:bg-destructive/10" />
const b = <><div className="border-dux-warning-x" /><p className="bg-card" /></>`,
    }
    expect(findHandBuiltNotices(quiet)).toEqual([])
  })
})
