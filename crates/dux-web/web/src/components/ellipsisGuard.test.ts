import { readFileSync } from "node:fs"
import { relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import { filesUnder, withoutCommentsLineStrings } from "@/test/sourceFiles"

// A hand-built `⋯` trigger drifts from the others in size, border, reveal and
// touch floor, so the dots glyph is reached by the shared trigger alone.
//
// This reads source text, so it catches what a line of source says and nothing
// the program does. It cannot see: a glyph from another icon library (a
// differently named dots icon, an inline SVG, three CSS dots); the glyph picked
// by a string at run time (`DynamicIcon name="ellipsis"`, `icons[name]`); a
// vertical-dots character or a run of periods used as a trigger's label; a
// trigger built without the glyph at all; a file outside `src/`; and a
// re-export through a file that is itself clean. A `⋯` counts only as a whole
// JSX text or a whole string (`>⋯<`, `"⋯"`), which is what a trigger's content
// is: the character inside a comment, or inside a sentence of prose that names
// the menu, is not one, and the comments and copy here name it all the time.

// Files that may reach the glyph, by path under `src/`, each with its reason.
const ALLOWED: Record<string, string> = {
  "components/EllipsisMenuTrigger.tsx": "the shared trigger itself",
  "components/ui/breadcrumb.tsx":
    "shadcn's collapsed-items marker: an aria-hidden span nothing renders, never a menu trigger",
}

const GLYPH_EXPORT = /^(?:Lucide)?(?:Ellipsis|EllipsisVertical|MoreHorizontal|MoreVertical)(?:Icon)?$/
// lucide's one-icon-per-file paths, e.g. "lucide-react/dist/esm/icons/ellipsis".
const GLYPH_PATH = /(?:^|\/)(?:ellipsis|ellipsis-vertical|more-horizontal|more-vertical)(?:\.[cm]?js)?$/
const GLYPH_CHARS = String.raw`(?:⋯|\\u22ef|\\u\{22ef\}|&#x22ef;|&#8943;)`
const LITERAL_GLYPH = new RegExp(
  String.raw`[>}]\s*${GLYPH_CHARS}\s*[<{]|(["'\x60])\s*${GLYPH_CHARS}\s*\1`,
  "i",
)


/** Why `src` reaches the dots glyph or draws one by hand, or null if it does not. */
export function glyphUse(src: string): string | null {
  const code = withoutCommentsLineStrings(src)
  // Every `from "lucide-react…"` and `import("lucide-react…")` and `require(…)`.
  const sources = [
    ...code.matchAll(
      /(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*)(["'])(lucide-react[^"']*)\1/g,
    ),
  ]
  for (const m of sources) {
    const from = m[2]
    if (from !== "lucide-react" && GLYPH_PATH.test(from)) return `imports ${from}`
    if (from === "lucide-react") {
      // The statement that holds this source, back to the keyword that opened it.
      const head = code.slice(0, m.index)
      const open = Math.max(head.lastIndexOf("import"), head.lastIndexOf("export"))
      const stmt = open >= 0 ? head.slice(open) : ""
      if (/\*\s*as\s/.test(stmt)) return "imports the whole lucide-react namespace"
      if (/\bicons\b|\bDynamicIcon\b/.test(stmt)) return "imports lucide's icon table"
      const named = stmt.match(/\{([^}]*)\}/)?.[1] ?? ""
      for (const n of named.split(",")) {
        const name = n.trim().split(/\s+as\s+/)[0].replace(/^type\s+/, "")
        if (GLYPH_EXPORT.test(name)) return `imports ${name}`
      }
      // `import("lucide-react")` and `require("lucide-react")` hand over the lot.
      if (!/^\s*(?:import|export)\b/.test(stmt) || stmt === "") {
        return "loads the whole lucide-react module"
      }
    }
  }
  if (LITERAL_GLYPH.test(code)) return "writes the ⋯ character"
  return null
}

describe("no hand-built ⋯ trigger (guard)", () => {
  it("reaches the ellipsis glyph in no file but the shared trigger", () => {
    const root = fileURLToPath(new URL("../", import.meta.url))
    const offenders = filesUnder(root)
      .filter((f) => /\.(?:[cm]?[jt]sx?)$/.test(f))
      .map((f) => [relative(root, f), glyphUse(readFileSync(f, "utf8"))] as const)
      .filter(([f, why]) => why !== null && !(f in ALLOWED) && f !== "components/ellipsisGuard.test.ts")
      .map(([f, why]) => `${f} ${why}`)
    expect(offenders, "build a ⋯ menu trigger with EllipsisMenuTrigger").toEqual([])
  })

  it("sees each way past it that reading text can see", () => {
    const sees = [
      'import { Ellipsis } from "lucide-react"',
      "import { Ellipsis } from 'lucide-react'",
      'import { MoreHorizontal as Dots } from "lucide-react"',
      'import { EllipsisVertical, X } from "lucide-react"',
      'import { EllipsisIcon } from "lucide-react"',
      'import { LucideEllipsis } from "lucide-react"',
      'import {\n  X,\n  MoreVertical,\n} from "lucide-react"',
      'import * as Icons from "lucide-react"',
      'import { icons } from "lucide-react"',
      'import Dots from "lucide-react/dist/esm/icons/ellipsis"',
      "import Dots from 'lucide-react/icons/more-horizontal.js'",
      'export { Ellipsis } from "lucide-react"',
      'const m = await import("lucide-react")',
      'const m = require("lucide-react")',
      '<Button>⋯</Button>',
      'const label = "\\u22EF"',
      "<button>&#x22EF;</button>",
    ]
    for (const src of sees) expect(glyphUse(src), src).not.toBeNull()
    const passes = [
      'import { X, Loader2 } from "lucide-react"',
      'import { Button } from "@/components/ui/button"',
      "// the ⋯ menu opens here\nconst a = 1",
      "/* the ⋯ menu */ const a = 1",
      'import { Dot } from "lucide-react" // not Ellipsis',
    ]
    for (const src of passes) expect(glyphUse(src), src).toBeNull()
  })
})
