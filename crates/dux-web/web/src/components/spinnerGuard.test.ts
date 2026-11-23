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

describe("guard: one loading indicator", () => {
  const files = Object.fromEntries(
    sourceFilesUnder(root).map((p) => [relative(root, p), readFileSync(p, "utf8")]),
  )

  it("draws every spinner through Spinner unless that occurrence is allowed with its reason", () => {
    const found = findSpinners(files)
    const unallowed = found
      .filter((f) => !ALLOWED.some((a) => a.file === f.file && a.line === f.line))
      .map((f) => `${f.file}: ${f.what}: ${f.line}`)
    expect(unallowed, "draw it with Spinner from components/Spinner").toEqual([])
    const stale = ALLOWED.filter(
      (a) => !found.some((f) => f.file === a.file && f.line === a.line),
    ).map((a) => `${a.file}: ${a.line}`)
    expect(stale, "an allowed occurrence is gone: remove its entry").toEqual([])
  })
})
