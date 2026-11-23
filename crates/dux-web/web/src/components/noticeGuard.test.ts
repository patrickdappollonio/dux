import { readdirSync, readFileSync } from "node:fs"
import { join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

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

const CLASS_CALL = /(?:\bcn|\bclsx|\bcva|\btwMerge|\bclassNames)\s*$/

// One string literal per entry, with the groups it belongs to.
function groupsOf(source: string): string[] {
  const groups: string[][] = []
  // The open brackets: the index of the group each one owns and whether it is a
  // class helper call.
  const stack: { group: number; classCall: boolean }[] = []
  const open = (classCall: boolean) => {
    groups.push([])
    stack.push({ group: groups.length - 1, classCall })
  }
  const add = (text: string, own: boolean) => {
    if (own || stack.length === 0) {
      groups.push([text])
      return
    }
    // Inside a class call the string belongs to that call and to every bracket
    // between it and the string; elsewhere to the innermost bracket alone.
    let top = stack.length - 1
    while (top >= 0 && !stack[top].classCall) top--
    const from = top >= 0 ? top : stack.length - 1
    for (let k = stack.length - 1; k >= from; k--) groups[stack[k].group].push(text)
  }
  let i = 0
  while (i < source.length) {
    const c = source[i]
    if (c === "/" && source[i + 1] === "/") {
      while (i < source.length && source[i] !== "\n") i++
      continue
    }
    if (c === "/" && source[i + 1] === "*") {
      const end = source.indexOf("*/", i + 2)
      i = end < 0 ? source.length : end + 2
      continue
    }
    if (c === '"' || c === "'" || c === "`") {
      let j = i + 1
      let closed = false
      while (j < source.length) {
        if (source[j] === "\\") {
          j += 2
          continue
        }
        if (source[j] === c) {
          closed = true
          break
        }
        // A quote that never closes on its line is an apostrophe in JSX text.
        if (c !== "`" && source[j] === "\n") break
        j++
      }
      if (closed) {
        const before = source.slice(Math.max(0, i - 40), i)
        const own = /[A-Za-z_$][\w$]*\s*=\s*$/.test(before) && !/[=!<>]=\s*$/.test(before)
        add(source.slice(i + 1, j), own)
        i = j + 1
      } else {
        i++
      }
      continue
    }
    if (c === "(" || c === "{" || c === "[") {
      open(c === "(" && CLASS_CALL.test(source.slice(Math.max(0, i - 12), i)))
    } else if (c === ")" || c === "}" || c === "]") {
      stack.pop()
    }
    i++
  }
  return groups.map((g) => g.join(" ").replace(/\s+/g, " ").trim()).filter(Boolean)
}

function sourcesUnder(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name)
    if (e.isDirectory()) return e.name === "node_modules" ? [] : sourcesUnder(p)
    return /\.tsx?$/.test(e.name) && !/\.test\.|\.d\.ts$/.test(e.name) ? [p] : []
  })
}

interface Finding {
  file: string
  look: Look
  text: string
}

function findHandBuiltNotices(files: Record<string, string>): Finding[] {
  const out: Finding[] = []
  for (const [file, source] of Object.entries(files)) {
    for (const text of groupsOf(source)) {
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
      sourcesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
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
