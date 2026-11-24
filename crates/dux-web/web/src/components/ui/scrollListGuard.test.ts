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
//   out: the edge hooks (`useScrollEdges`, `useScrollEdgesOf`), the attributes
//   the stylesheet keys the shadow on (`data-scroll-list-fade`,
//   `data-more-above`, `data-more-below`) or the old class (`dux-list-fade`);
// - "masked-scroller": one class group that both scrolls (`overflow-auto`,
//   `overflow-y-auto`, `overflow-scroll`, at any breakpoint) and masks (any
//   Tailwind `mask-` utility);
// - "mask-style": a mask set from a style object (`maskImage`).
//
// It reads text, not the program: it cannot see a shadow painted as a gradient
// overlay on a sibling of the scroller, one written in a stylesheet other than
// through these attributes, a mask whose class sits in a different group from
// the scroller's, or one built at runtime from pieces. Exceptions are allowed
// by exact occurrence, with a reason; there are none.

const root = fileURLToPath(new URL("../../", import.meta.url))

// The component and the hook it measures with are where the shadow lives.
const HOME = new Set(["components/ui/scroll-list.tsx", "hooks/use-scroll-edges.ts"])

type Look = "edge-marker" | "masked-scroller" | "mask-style"

const MARKER =
  /\b(?:useScrollEdges|useScrollEdgesOf)\b|data-scroll-list-fade|data-more-above|data-more-below|dux-list-fade/g
const token = (re: string) => new RegExp(`(?:^|\\s)(?:[\\w-]+:)*${re}`)
const SCROLLS = token("overflow(?:-y)?-(?:auto|scroll)(?=\\s|$)")
const MASKS = token("mask-\\S+")
const MASK_STYLE = /\b(?:Webkit|webkit)?[mM]askImage\b/g

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
      "hook.tsx": "const { ref, moreBelow } = useScrollEdges<HTMLDivElement>()",
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
      "ok.tsx": `// a list that fades (useScrollEdges, data-more-below) is a ScrollList
const a = <div className="overflow-y-auto px-2" />
const b = <div className="mask-b-from-80%" />
const c = <ScrollList scroll="own" height="fill" scrollbar="hidden" />`,
    }
    expect(findHandBuiltEdgeShadows(quiet)).toEqual([])
  })
})
