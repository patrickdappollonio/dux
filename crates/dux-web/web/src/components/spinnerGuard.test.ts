import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { sourceFilesUnder, withoutComments } from "@/test/sourceFiles"

// A loading indicator is the `Spinner` component (components/Spinner.tsx), in
// its glyph or its ring kind: a second spinner is a second place to forget
// reduced motion, the fixed glyph slot or the screen reader. This guard reads
// every source file under `src` (all folders, .ts and .tsx, tests excluded),
// with its comments removed, and fails on a line that draws a spinner itself:
//
// - any lucide loader icon, by name (`Loader`, `Loader2`, `Loader2Icon`,
//   `LoaderCircle`, `LucideLoader2` and the rest), wherever the name appears,
//   so an import, a renamed import and `Icons.Loader2` are all seen;
// - a spin animation in a class (`animate-spin`, `animate-[spin…]`) or in an
//   inline `animation` style;
// - the glyph spinner's frames, its slot class or its arc characters.
//
// It reads text, not the program. It cannot see a spinner whose icon is not
// called a loader (any other icon given a spin class is seen by its class), a
// spin built from parts at runtime, one set in a stylesheet (index.css), an
// animated SVG or image, or a timer that cycles characters other than the arcs.

const root = fileURLToPath(new URL("../", import.meta.url))

// The component and the frames it draws.
const HOME = new Set(["components/Spinner.tsx", "lib/spinnerFrames.ts"])

const PATTERNS: { what: string; re: RegExp }[] = [
  { what: "a lucide loader icon", re: /\b(?:Lucide)?Loader[A-Z0-9]\w*|\b(?:Lucide)?Loader\b/ },
  { what: "a spin animation class", re: /animate-(?:spin\b|\[spin)/ },
  { what: "a spin animation style", re: /animation[\w-]*["']?\s*[:=][^\n]*\bspin\b/ },
  {
    what: "the glyph spinner's frames",
    re: /SPINNER_FRAMES|GLYPH_SPINNER_CLASS|glyph-spinner|[◜-◡]|\\u25d[c-f]|\\u25e[01]|\\u\{25d[c-f]\}|\\u\{25e[01]\}/i,
  },
]

// Each allowed occurrence by its file and the exact text of its line.
const ALLOWED: { file: string; line: string; reason: string }[] = [
  {
    file: "components/ui/sonner.tsx",
    line: 'import { CircleCheckIcon, InfoIcon, TriangleAlertIcon, OctagonXIcon, Loader2Icon } from "lucide-react"',
    reason: "The icon the toast library shows on a busy toast: a busy toast's spinner is the toast library's.",
  },
  {
    file: "components/ui/sonner.tsx",
    line: 'loading: "size-4 animate-spin text-muted-foreground",',
    reason: "The same busy-toast icon's classes.",
  },
  {
    file: "components/ui/sonner.tsx",
    line: "<Loader2Icon className={TONE_ICON.loading} />",
    reason: "The same busy-toast icon, handed to the toast library.",
  },
]

function findSpinners(files: Record<string, string>): { file: string; line: string; what: string }[] {
  const out: { file: string; line: string; what: string }[] = []
  for (const [file, source] of Object.entries(files)) {
    if (HOME.has(file)) continue
    for (const line of withoutComments(source).split("\n")) {
      const hit = PATTERNS.find((p) => p.re.test(line))
      if (hit) out.push({ file, line: line.trim(), what: hit.what })
    }
  }
  return out
}

// Match each occurrence to one allowed entry; what is left over on either side
// is wrong, so an entry covers exactly one occurrence of its line.
function unmatched(
  found: { file: string; line: string; what: string }[],
  allowed: { file: string; line: string }[],
): { unallowed: string[]; stale: string[] } {
  const free = [...allowed]
  const unallowed: string[] = []
  for (const f of found) {
    const at = free.findIndex((a) => a.file === f.file && a.line === f.line)
    if (at === -1) unallowed.push(`${f.file}: ${f.what}: ${f.line}`)
    else free.splice(at, 1)
  }
  return { unallowed, stale: free.map((a) => `${a.file}: ${a.line}`) }
}

describe("guard: one loading indicator", () => {
  const files = Object.fromEntries(
    sourceFilesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
  )

  it("draws every spinner through Spinner unless that occurrence is allowed with its reason", () => {
    const { unallowed, stale } = unmatched(findSpinners(files), ALLOWED)
    expect(unallowed, "draw it with Spinner from components/Spinner").toEqual([])
    expect(stale, "an allowed occurrence is gone: remove its entry").toEqual([])
  })

  it("sees the glyph spinner's frames and a spin style, and ignores other text", () => {
    const seen = (source: string) => findSpinners({ "components/X.tsx": source }).map((f) => f.what)
    expect(seen('const f = "◜◝◞◟"')).toEqual(["the glyph spinner's frames"])
    expect(seen('const f = "\\u25dc"')).toEqual(["the glyph spinner's frames"])
    expect(seen("import { SPINNER_FRAMES } from \"@/lib/spinnerFrames\"")).toEqual([
      "the glyph spinner's frames",
    ])
    expect(seen('<span className="glyph-spinner" />')).toEqual(["the glyph spinner's frames"])
    expect(seen('<i style={{ animation: "spin 1s linear infinite" }} />')).toEqual([
      "a spin animation style",
    ])
    expect(seen('<i style={{ animationName: "spin" }} />')).toEqual(["a spin animation style"])
    expect(seen('<i className="animate-spin" />')).toEqual(["a spin animation class"])
    expect(seen('<i style={{ animation: "fade 1s" }} />')).toEqual([])
    expect(seen('<i className="animate-pulse spinner-like" />')).toEqual([])
    expect(seen("// animate-spin ◜")).toEqual([])
    expect(findSpinners({ "components/Spinner.tsx": 'const f = "◜◝◞◟"' })).toEqual([])
  })

  it("holds one allowed entry to one occurrence", () => {
    const entry = { file: "components/X.tsx", line: "<Loader2 />", reason: "x" }
    const hit = { file: "components/X.tsx", line: "<Loader2 />", what: "a lucide loader icon" }
    expect(unmatched([hit, hit], [entry]).unallowed).toHaveLength(1)
    expect(unmatched([hit], [entry, entry]).stale).toHaveLength(1)
    expect(unmatched([hit], [entry])).toEqual({ unallowed: [], stale: [] })
  })
})
