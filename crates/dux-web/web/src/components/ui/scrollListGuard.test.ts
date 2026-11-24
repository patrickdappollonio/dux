import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { classGroupsOf } from "@/test/classGroups"
import { sourceFilesUnder, withoutComments } from "@/test/sourceFiles"

// A list that scrolls says there is more past an edge through `ScrollList`
// (components/ui/scroll-list.tsx), which owns the box, the shadow at each edge
// and the scroll padding that keeps a revealed row clear of it. This guard
// reads every source file under `src` (tests excluded) and fails on an edge
// shadow built anywhere else:
//
// - "edge-marker": the shadow's own parts reached directly, with comments taken
//   out: the edge hook (`useScrollEdgesOf`), the answers it is built from
//   (`hasMoreAbove`, `hasMoreBelow`), the attributes the stylesheet keys the
//   shadow on (`data-scroll-list-fade`, `data-scroll-list-box`,
//   `data-scroll-list-flush`, `data-more-above`, `data-more-below`) or the old
//   class (`dux-list-fade`);
// - "masked-scroller": one class group that both scrolls (`overflow-auto`,
//   `overflow-y-auto`, `overflow-scroll`, at any breakpoint) and masks (a
//   Tailwind `mask-` utility or an arbitrary property such as `[mask-image:...]`
//   or `[-webkit-mask:...]`);
// - "mask-style": a mask set in code: a style object's `maskImage`, `mask` or
//   `WebkitMask`, or `"mask-image"` and `"mask"` as a quoted name.
//
// It reads text, not the program: it cannot see a shadow painted as a gradient
// overlay on a sibling of the scroller, one written in a stylesheet other than
// through these attributes, a mask whose class sits in a different group from
// the scroller's, a mask whose property name is assembled at runtime, or a
// `ScrollList` that is imported and then not rendered. That the lists use the
// container is the ADOPTERS check below, which reads text too: a file that
// names `<ScrollList`, not proof that the list inside it is the one that
// scrolls. Exceptions are allowed by exact occurrence, with a reason; there are
// none.

const root = fileURLToPath(new URL("../../", import.meta.url))

// The component and the hook it measures with are where the shadow lives.
const HOME = new Set(["components/ui/scroll-list.tsx", "hooks/use-scroll-edges.ts"])

// The one other reader of the edge answers: the selected-row reveal asks whether
// an edge's shadow is showing to know how much of the box a row must clear. It
// reads the answers and paints nothing.
const READS_EDGES = new Set(["hooks/use-reveal-selected-row.ts"])

type Look = "edge-marker" | "masked-scroller" | "mask-style"

const MARKER =
  /\b(?:useScrollEdges|useScrollEdgesOf)\b|data-scroll-list-(?:fade|box|flush)|data-more-above|data-more-below|dux-list-fade/g
const EDGE_ANSWERS = /\bhasMore(?:Above|Below)\b/g
const token = (re: string) => new RegExp(`(?:^|\\s)(?:[\\w-]+:)*${re}`)
const SCROLLS = token("overflow(?:-y)?-(?:auto|scroll)(?=\\s|$)")
const MASKS = token("(?:mask-\\S+|\\[(?:-webkit-)?mask(?:-[\\w-]+)?:)")
const MASK_STYLE =
  /\b(?:Webkit|webkit)?[mM]askImage\b|["']-?(?:webkit-)?mask(?:-image)?["']|\b(?:Webkit|webkit)?[mM]ask\s*:/g

interface Finding {
  file: string
  look: Look
  text: string
}

function findHandBuiltEdgeShadows(files: Record<string, string>): Finding[] {
  const out: Finding[] = []
  for (const [file, source] of Object.entries(files)) {
    if (HOME.has(file)) continue
    const code = withoutComments(source)
    for (const m of code.matchAll(MARKER)) out.push({ file, look: "edge-marker", text: m[0] })
    if (!READS_EDGES.has(file)) {
      for (const m of code.matchAll(EDGE_ANSWERS)) out.push({ file, look: "edge-marker", text: m[0] })
    }
    for (const m of code.matchAll(MASK_STYLE)) out.push({ file, look: "mask-style", text: m[0] })
    for (const text of classGroupsOf(source)) {
      if (SCROLLS.test(text) && MASKS.test(text)) out.push({ file, look: "masked-scroller", text })
    }
  }
  return out
}

// One entry per exact occurrence: the file, the look, a piece of the text that
// identifies it and how many times it occurs. An entry that matches fewer times
// than `count` fails the suite, and one more occurrence than `count` is a
// finding.
const ALLOWED: { file: string; look: Look; contains: string; count: number; reason: string }[] = []

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

describe("a list's edge shadow goes through the shared ScrollList", () => {
  it("finds no hand-built edge shadow in any folder under src", () => {
    const files = Object.fromEntries(
      sourceFilesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
    )
    const { rest, unused } = unallowed(findHandBuiltEdgeShadows(files))
    expect(
      rest.map((f) => `${f.file} (${f.look}): ${f.text}`),
      "use ScrollList from components/ui/scroll-list, or allow the exact occurrence with a reason",
    ).toEqual([])
    expect(unused, "an allowed occurrence that no longer exists: remove its entry").toEqual([])
  })

  it("sees the ways a hand-built shadow can be written", () => {
    const cases: Record<string, string> = {
      "hook.tsx": "const { moreBelow } = useScrollEdgesOf(node)",
      "answers.ts": "const below = hasMoreBelow(el)",
      "arbitrary.tsx": `const E = () => <div className="overflow-y-auto [mask-image:linear-gradient(#000,transparent)]" />`,
      "webkit.tsx": `const F = () => <div className={cn("overflow-auto", "[-webkit-mask:var(--fade)]")} />`,
      "shorthand.tsx": `const G = () => <div style={{ mask: fade }} />`,
      "quoted.ts": `el.style.setProperty("mask-image", fade)`,
      "attr.tsx": `const A = () => <div data-more-below={below ? "" : undefined} />`,
      "class.tsx": `const B = () => <div className="dux-list-fade overflow-y-auto" />`,
      "set.ts": `el.setAttribute("data-scroll-list-fade", "")`,
      "mask.tsx": `const C = () => <div className={cn("min-h-0 md:overflow-y-auto", "mask-b-from-80%")} />`,
      "style.tsx": `const D = () => <div style={{ WebkitMaskImage: fade }} />`,
    }
    const hit = new Set(findHandBuiltEdgeShadows(cases).map((f) => f.file))
    expect([...hit].sort()).toEqual(Object.keys(cases).sort())
  })

  it("leaves the component, comments and unrelated classes alone", () => {
    const quiet = {
      "components/ui/scroll-list.tsx": `const x = useScrollEdgesOf(node); el.setAttribute("data-more-above", "")`,
      "hooks/use-scroll-edges.ts": "export function useScrollEdges() {}",
      "hooks/use-reveal-selected-row.ts": "const a = hasMoreAbove(el) && hasMoreBelow(el)",
      "ok.tsx": `// a list that fades (useScrollEdges, data-more-below) is a ScrollList
const mask = "the word mask in copy"
const a = <div className="overflow-y-auto px-2" />
const b = <div className="mask-b-from-80%" />
const c = <ScrollList scroll="own" height="fill" scrollbar="hidden" />`,
    }
    expect(findHandBuiltEdgeShadows(quiet)).toEqual([])
  })
})

// The lists that were moved onto the container. Each must still render a
// `<ScrollList`, so a list moved back to a hand-made scroller (which the checks
// above may not see) fails here.
const ADOPTERS = [
  "components/FlatAgentList.tsx",
  "components/ProjectShortcutList.tsx",
  "components/ProjectsDialog.tsx",
  "components/NewAgentPickerDialog.tsx",
  "components/WorktreesDialog.tsx",
  "components/PullRequestsDialog.tsx",
  "components/MacrosDialog.tsx",
]

describe("the lists that moved onto ScrollList still use it", () => {
  it.each(ADOPTERS)("%s renders a ScrollList", (file) => {
    const code = withoutComments(readFileSync(`${root}${file}`, "utf8"))
    expect(code, `${file} no longer renders <ScrollList`).toMatch(/<ScrollList\b/)
  })
})
